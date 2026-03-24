#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

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

#[test]
fn sighup_rejects_accept_worker_changes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "stable workers\n");

    server.wait_for_body(listen_addr, "stable workers\n", Duration::from_secs(5));

    server.write_config(static_config_with_runtime(
        listen_addr,
        "should not apply\n",
        "        accept_workers: Some(2),\n",
    ));
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(listen_addr, "stable workers\n", Duration::from_secs(5));
    server.kill_and_wait(Duration::from_secs(5));
}

#[test]
fn sighup_rejects_runtime_worker_thread_changes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "stable runtime\n");

    server.wait_for_body(listen_addr, "stable runtime\n", Duration::from_secs(5));

    server.write_config(static_config_with_runtime(
        listen_addr,
        "should not apply\n",
        "        worker_threads: Some(2),\n",
    ));
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(listen_addr, "stable runtime\n", Duration::from_secs(5));
    server.kill_and_wait(Duration::from_secs(5));
}

#[test]
fn sighup_reload_picks_up_updated_included_fragments() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn_with_setup("rginx-reload-include-test", |temp_dir| {
        fs::write(temp_dir.join("routes.ron"), static_route_fragment("before include reload\n"))
            .expect("initial routes fragment should be written");
        format!(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        // @include \"routes.ron\"\n    ],\n)\n",
            listen_addr.to_string(),
            ready_route = READY_ROUTE_CONFIG,
        )
    });
    let routes_path = server.temp_dir().join("routes.ron");

    server.wait_for_body(listen_addr, "before include reload\n", Duration::from_secs(5));

    fs::write(&routes_path, static_route_fragment("after include reload\n"))
        .expect("updated routes fragment should be written");
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(listen_addr, "after include reload\n", Duration::from_secs(5));
    server.shutdown_and_wait(Duration::from_secs(5));
}

struct TestServer {
    inner: ServerHarness,
}

impl TestServer {
    fn spawn(listen_addr: SocketAddr, body: &str) -> Self {
        Self::spawn_with_config("rginx-test", static_config(listen_addr, body))
    }

    fn spawn_with_config(prefix: &str, config: String) -> Self {
        Self::spawn_with_setup(prefix, |_| config)
    }

    fn spawn_with_setup(prefix: &str, setup: impl FnOnce(&Path) -> String) -> Self {
        Self { inner: ServerHarness::spawn(prefix, setup) }
    }

    fn write_static_config(&self, listen_addr: SocketAddr, body: &str) {
        write_static_config(self.inner.config_path(), listen_addr, body);
    }

    fn write_config(&self, config: String) {
        fs::write(self.inner.config_path(), config).expect("config file should be written");
    }

    fn wait_for_body(&mut self, listen_addr: SocketAddr, expected: &str, timeout: Duration) {
        self.inner.wait_for_http_text_response(
            listen_addr,
            &listen_addr.to_string(),
            "/",
            200,
            expected,
            timeout,
        );
    }

    fn send_signal(&self, signal: i32) {
        self.inner.send_signal(signal);
    }

    fn shutdown_and_wait(&mut self, timeout: Duration) {
        self.inner.terminate_and_wait(timeout);
    }

    fn kill_and_wait(&mut self, timeout: Duration) {
        self.inner.kill_and_wait(timeout);
    }

    fn temp_dir(&self) -> &Path {
        self.inner.temp_dir()
    }
}

fn write_static_config(path: &Path, listen_addr: SocketAddr, body: &str) {
    fs::write(path, static_config(listen_addr, body)).expect("config file should be written");
}

fn static_config(listen_addr: SocketAddr, body: &str) -> String {
    static_config_with_runtime(listen_addr, body, "")
}

fn static_config_with_runtime(listen_addr: SocketAddr, body: &str, runtime_extra: &str) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n{}    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Static(\n                status: Some(200),\n                content_type: Some(\"text/plain; charset=utf-8\"),\n                body: {:?},\n            ),\n        ),\n    ],\n)\n",
        runtime_extra,
        listen_addr.to_string(),
        body,
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn static_route_fragment(body: &str) -> String {
    format!(
        "LocationConfig(\n    matcher: Exact(\"/\"),\n    handler: Static(\n        status: Some(200),\n        content_type: Some(\"text/plain; charset=utf-8\"),\n        body: {:?},\n    ),\n),\n",
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
    let deadline = std::time::Instant::now() + timeout;

    while std::time::Instant::now() < deadline {
        match fetch_text_response(listen_addr, "/") {
            Ok((status, body)) => {
                panic!(
                    "expected {} to stay unreachable, got status={} body={:?}",
                    listen_addr, status, body
                );
            }
            Err(_) => {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
