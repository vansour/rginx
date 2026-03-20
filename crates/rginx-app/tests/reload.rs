#![cfg(unix)]

use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[test]
fn sighup_reload_applies_updated_routes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "before reload\n");

    server.wait_for_body(listen_addr, "before reload\n", Duration::from_secs(5));

    server.write_static_config(listen_addr, "after reload\n");
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(listen_addr, "after reload\n", Duration::from_secs(5));
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn sighup_rejects_listen_address_changes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let initial_addr = reserve_loopback_addr();
    let rejected_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(initial_addr, "stable config\n");

    server.wait_for_body(initial_addr, "stable config\n", Duration::from_secs(5));

    server.write_static_config(rejected_addr, "should not apply\n");
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(initial_addr, "stable config\n", Duration::from_secs(5));
    assert_unreachable(rejected_addr, Duration::from_millis(500));

    server.shutdown_and_wait(Duration::from_secs(5));
}

struct TestServer {
    child: Child,
    config_path: PathBuf,
    temp_dir: PathBuf,
}

impl TestServer {
    fn spawn(listen_addr: SocketAddr, body: &str) -> Self {
        let temp_dir = temp_dir("rginx-test");
        fs::create_dir_all(&temp_dir).expect("temp test dir should be created");
        let config_path = temp_dir.join("rginx.ron");
        write_static_config(&config_path, listen_addr, body);

        let child = Command::new(binary_path())
            .arg("--config")
            .arg(&config_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("rginx should start");

        Self { child, config_path, temp_dir }
    }

    fn write_static_config(&self, listen_addr: SocketAddr, body: &str) {
        write_static_config(&self.config_path, listen_addr, body);
    }

    fn wait_for_body(&mut self, listen_addr: SocketAddr, expected: &str, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        let mut last_error = String::new();

        while Instant::now() < deadline {
            self.assert_running();

            match fetch_text_response(listen_addr, "/") {
                Ok((status, body)) if status == 200 && body == expected => return,
                Ok((status, body)) => {
                    last_error = format!(
                        "unexpected response from {listen_addr}: status={status} body={body:?}"
                    );
                }
                Err(error) => last_error = error,
            }

            thread::sleep(Duration::from_millis(50));
        }

        panic!(
            "timed out waiting for response body {:?} on {}; last error: {}",
            expected, listen_addr, last_error
        );
    }

    fn send_signal(&self, signal: i32) {
        let result = unsafe { libc::kill(self.child.id() as i32, signal) };
        if result != 0 {
            panic!(
                "failed to send signal {} to pid {}: {}",
                signal,
                self.child.id(),
                std::io::Error::last_os_error()
            );
        }
    }

    fn shutdown_and_wait(&mut self, timeout: Duration) {
        self.send_signal(libc::SIGTERM);
        let status = self.wait_for_exit(timeout);
        assert!(status.success(), "rginx should exit successfully, got {status}");
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;

        loop {
            if let Some(status) = self.child.try_wait().expect("child status should be readable") {
                return status;
            }

            if Instant::now() >= deadline {
                let _ = unsafe { libc::kill(self.child.id() as i32, libc::SIGKILL) };
                let _ = self.child.wait();
                panic!("timed out waiting for rginx to exit");
            }

            thread::sleep(Duration::from_millis(50));
        }
    }

    fn assert_running(&mut self) {
        if let Some(status) = self.child.try_wait().expect("child status should be readable") {
            panic!("rginx exited unexpectedly with status {status}");
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = unsafe { libc::kill(self.child.id() as i32, libc::SIGKILL) };
            let _ = self.child.wait();
        }

        let _ = fs::remove_dir_all(&self.temp_dir);
    }
}

fn write_static_config(path: &Path, listen_addr: SocketAddr, body: &str) {
    fs::write(path, static_config(listen_addr, body)).expect("config file should be written");
}

fn static_config(listen_addr: SocketAddr, body: &str) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Static(\n                status: Some(200),\n                content_type: Some(\"text/plain; charset=utf-8\"),\n                body: {:?},\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        body
    )
}

fn fetch_text_response(listen_addr: SocketAddr, path: &str) -> Result<(u16, String), String> {
    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;

    write!(stream, "GET {path} HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n")
        .map_err(|error| format!("failed to write request: {error}"))?;
    stream.flush().map_err(|error| format!("failed to flush request: {error}"))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| format!("failed to read response: {error}"))?;

    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| format!("malformed response: {response:?}"))?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| format!("missing status line: {head:?}"))?
        .parse::<u16>()
        .map_err(|error| format!("invalid status code: {error}"))?;

    Ok((status, body.to_string()))
}

fn assert_unreachable(listen_addr: SocketAddr, timeout: Duration) {
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        match fetch_text_response(listen_addr, "/") {
            Ok((status, body)) => {
                panic!(
                    "expected {} to stay unreachable, got status={} body={:?}",
                    listen_addr, status, body
                );
            }
            Err(_) => {
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn reserve_loopback_addr() -> SocketAddr {
    let listener =
        TcpListener::bind(("127.0.0.1", 0)).expect("ephemeral loopback listener should bind");
    let addr = listener.local_addr().expect("listener addr should be available");
    drop(listener);
    addr
}

fn temp_dir(prefix: &str) -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    env::temp_dir().join(format!("{prefix}-{unique}-{id}"))
}

fn binary_path() -> PathBuf {
    env::var_os("CARGO_BIN_EXE_rginx")
        .map(PathBuf::from)
        .expect("cargo should expose the rginx test binary path")
}

fn test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
