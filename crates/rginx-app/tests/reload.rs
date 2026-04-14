#![cfg(unix)]

#[allow(unused_imports)]
use std::fs;
#[allow(unused_imports)]
use std::io::{Read, Write};
#[allow(unused_imports)]
use std::net::{SocketAddr, TcpStream};
#[allow(unused_imports)]
use std::path::{Path, PathBuf};
#[allow(unused_imports)]
use std::process::{Command, Output};
#[allow(unused_imports)]
use std::sync::mpsc;
#[allow(unused_imports)]
use std::sync::{Mutex, OnceLock};
#[allow(unused_imports)]
use std::time::{Duration, Instant};

#[allow(unused_imports)]
use rcgen::{CertifiedKey, KeyPair};

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[path = "reload/reload_boundary.rs"]
mod reload_boundary;
#[path = "reload/reload_flow.rs"]
mod reload_flow;
#[path = "reload/restart_flow.rs"]
mod restart_flow;
#[path = "reload/streaming_flow.rs"]
mod streaming_flow;

struct TestServer {
    inner: ServerHarness,
}

impl TestServer {
    fn spawn(listen_addr: SocketAddr, body: &str) -> Self {
        Self::spawn_with_config("rginx-test", return_config(listen_addr, body))
    }

    fn spawn_with_config(prefix: &str, config: String) -> Self {
        Self::spawn_with_setup(prefix, |_| config)
    }

    fn spawn_with_setup(prefix: &str, setup: impl FnOnce(&Path) -> String) -> Self {
        Self { inner: ServerHarness::spawn(prefix, setup) }
    }

    fn write_return_config(&self, listen_addr: SocketAddr, body: &str) {
        write_return_config(self.inner.config_path(), listen_addr, body);
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

    fn wait_for_http_ready(&mut self, listen_addr: SocketAddr, timeout: Duration) {
        self.inner.wait_for_http_ready(listen_addr, timeout);
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

    fn wait_for_exit(&mut self, timeout: Duration) -> std::process::ExitStatus {
        self.inner.wait_for_exit(timeout)
    }

    fn temp_dir(&self) -> &Path {
        self.inner.temp_dir()
    }

    fn pid_path(&self) -> PathBuf {
        self.inner.config_path().with_extension("pid")
    }

    fn send_cli_signal(&self, signal: &str) -> Output {
        Command::new(binary_path())
            .arg("--config")
            .arg(self.inner.config_path())
            .arg("-s")
            .arg(signal)
            .output()
            .expect("rginx signal command should run")
    }

    fn run_cli_command<'a>(&self, args: impl IntoIterator<Item = &'a str>) -> Output {
        let mut command = Command::new(binary_path());
        command.arg("--config").arg(self.inner.config_path());
        for arg in args {
            command.arg(arg);
        }
        command.output().expect("rginx command should run")
    }

    fn wait_for_status_output(
        &self,
        predicate: impl Fn(&str) -> bool,
        timeout: Duration,
    ) -> String {
        wait_for_status_output(self.inner.config_path(), predicate, timeout)
    }
}

fn write_return_config(path: &Path, listen_addr: SocketAddr, body: &str) {
    fs::write(path, return_config(listen_addr, body)).expect("config file should be written");
}

fn return_config(listen_addr: SocketAddr, body: &str) -> String {
    return_config_with_runtime(listen_addr, body, "")
}

fn return_config_with_runtime(listen_addr: SocketAddr, body: &str, runtime_extra: &str) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n{}    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some({:?}),\n            ),\n        ),\n    ],\n)\n",
        runtime_extra,
        listen_addr.to_string(),
        body,
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn return_route_fragment(body: &str) -> String {
    format!(
        "LocationConfig(\n    matcher: Exact(\"/\"),\n    handler: Return(\n        status: 200,\n        location: \"\",\n        body: Some({:?}),\n    ),\n),\n",
        body
    )
}

fn explicit_listeners_config(listeners: &[(&str, SocketAddr)], body: &str) -> String {
    let listeners = listeners
        .iter()
        .map(|(name, addr)| {
            format!(
                "        ListenerConfig(\n            name: {:?},\n            listen: {:?},\n        )",
                name,
                addr.to_string()
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");

    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    listeners: [\n{listeners}\n    ],\n    server: ServerConfig(\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some({body:?}),\n            ),\n        ),\n    ],\n)\n",
        listeners = listeners,
        ready_route = READY_ROUTE_CONFIG,
        body = body,
    )
}

fn explicit_listeners_proxy_config(
    listeners: &[(&str, SocketAddr)],
    upstream_addr: SocketAddr,
) -> String {
    let listeners = listeners
        .iter()
        .map(|(name, addr)| {
            format!(
                "        ListenerConfig(\n            name: {:?},\n            listen: {:?},\n        )",
                name,
                addr.to_string()
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");

    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    listeners: [\n{listeners}\n    ],\n    server: ServerConfig(\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {upstream:?},\n                ),\n            ],\n            request_timeout_secs: Some(3),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listeners = listeners,
        upstream = format!("http://{upstream_addr}"),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn fetch_text_response(listen_addr: SocketAddr, path: &str) -> Result<(u16, String), String> {
    fetch_text_response_with_timeout(listen_addr, path, Duration::from_millis(500))
}

fn fetch_text_response_with_timeout(
    listen_addr: SocketAddr,
    path: &str,
    read_timeout: Duration,
) -> Result<(u16, String), String> {
    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(read_timeout))
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

fn spawn_delayed_response_server(
    delay: Duration,
    body: &'static str,
    notify_ready: Option<mpsc::Sender<()>>,
) -> SocketAddr {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .expect("delayed upstream listener should bind");
    let listen_addr = listener.local_addr().expect("listener addr should be available");

    std::thread::spawn(move || {
        loop {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let notify_ready = notify_ready.clone();

            std::thread::spawn(move || {
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);
                if let Some(notify_ready) = &notify_ready {
                    let _ = notify_ready.send(());
                }
                std::thread::sleep(delay);

                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            });
        }
    });

    listen_addr
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

fn wait_for_body(listen_addr: SocketAddr, expected: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let mut last_error = String::new();

    while Instant::now() < deadline {
        match fetch_text_response(listen_addr, "/") {
            Ok((200, body)) if body == expected => return,
            Ok((status, body)) => {
                last_error = format!("unexpected response: status={status} body={body:?}");
            }
            Err(error) => last_error = error,
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    panic!(
        "timed out waiting for expected response on {}; expected body {:?}; last error: {}",
        listen_addr, expected, last_error
    );
}

fn read_pid_file(path: &Path) -> i32 {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read pid file {}: {error}", path.display()))
        .trim()
        .parse::<i32>()
        .unwrap_or_else(|error| panic!("invalid pid file {}: {error}", path.display()))
}

fn wait_for_pid_change(path: &Path, old_pid: i32, timeout: Duration) -> i32 {
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        if path.exists() {
            let pid = read_pid_file(path);
            if pid != old_pid {
                return pid;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    panic!("timed out waiting for pid file {} to move away from pid {}", path.display(), old_pid);
}

fn wait_for_process_exit(pid: i32, timeout: Duration) {
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        let result = unsafe { libc::kill(pid, 0) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return;
            }
        }

        std::thread::sleep(Duration::from_millis(50));
    }

    panic!("timed out waiting for pid {pid} to exit");
}

fn binary_path() -> std::path::PathBuf {
    std::env::var_os("CARGO_BIN_EXE_rginx")
        .map(std::path::PathBuf::from)
        .expect("cargo should expose the rginx test binary path")
}

fn render_output(output: &Output) -> String {
    format!(
        "status={}; stdout={:?}; stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

type TestCertifiedKey = CertifiedKey<KeyPair>;

fn generate_cert(hostname: &str) -> TestCertifiedKey {
    rcgen::generate_simple_self_signed(vec![hostname.to_string()])
        .expect("self-signed certificate should generate")
}

fn tls_return_config(listen_addr: SocketAddr, cert_path: &Path, key_path: &Path) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"tls reload\\n\"),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn run_cli_command<'a>(config_path: &Path, args: impl IntoIterator<Item = &'a str>) -> Output {
    let mut command = Command::new(binary_path());
    command.arg("--config").arg(config_path);
    for arg in args {
        command.arg(arg);
    }
    command.output().expect("rginx command should run")
}

fn wait_for_status_output(
    config_path: &Path,
    predicate: impl Fn(&str) -> bool,
    timeout: Duration,
) -> String {
    let deadline = Instant::now() + timeout;
    let mut last_output = String::new();

    while Instant::now() < deadline {
        let output = run_cli_command(config_path, ["status"]);
        let rendered = render_output(&output);

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            if predicate(&stdout) {
                return stdout;
            }
        }

        last_output = rendered;
        std::thread::sleep(Duration::from_millis(50));
    }

    panic!(
        "timed out waiting for rginx status on {} to satisfy the expected condition; last output: {}",
        config_path.display(),
        last_output
    );
}

fn test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
