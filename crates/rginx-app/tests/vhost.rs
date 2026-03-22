use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[test]
fn routes_requests_by_host_and_path_end_to_end() {
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, vhost_config(listen_addr));

    server.wait_for_response(
        listen_addr,
        "default.example.com",
        "/",
        200,
        "default root\n",
        Duration::from_secs(5),
    );
    server.wait_for_response(
        listen_addr,
        "unknown.example.com",
        "/",
        200,
        "default root\n",
        Duration::from_secs(5),
    );
    server.wait_for_response(
        listen_addr,
        "api.example.com",
        "/users",
        200,
        "api users\n",
        Duration::from_secs(5),
    );
    server.wait_for_response(
        listen_addr,
        &format!("api.example.com:{}", listen_addr.port()),
        "/",
        200,
        "api root\n",
        Duration::from_secs(5),
    );
    server.wait_for_response(
        listen_addr,
        "app.internal.example.com",
        "/",
        200,
        "internal root\n",
        Duration::from_secs(5),
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn matched_vhost_does_not_fall_back_to_default_routes_end_to_end() {
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, no_fallback_config(listen_addr));

    server.wait_for_response(
        listen_addr,
        "default.example.com",
        "/users",
        200,
        "default users\n",
        Duration::from_secs(5),
    );
    server.wait_for_response(
        listen_addr,
        "api.example.com",
        "/users",
        404,
        "route not found\n",
        Duration::from_secs(5),
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

struct TestServer {
    child: Child,
    temp_dir: PathBuf,
}

impl TestServer {
    fn spawn(listen_addr: SocketAddr, config: String) -> Self {
        let temp_dir = temp_dir("rginx-vhost-test");
        fs::create_dir_all(&temp_dir).expect("temp test dir should be created");
        let config_path = temp_dir.join("rginx.ron");
        fs::write(&config_path, config).expect("vhost config should be written");

        let child = Command::new(binary_path())
            .arg("--config")
            .arg(&config_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("rginx should start");

        let _ = listen_addr;
        Self { child, temp_dir }
    }

    fn wait_for_response(
        &mut self,
        listen_addr: SocketAddr,
        host: &str,
        path: &str,
        expected_status: u16,
        expected_body: &str,
        timeout: Duration,
    ) {
        let deadline = Instant::now() + timeout;
        let mut last_error = String::new();

        while Instant::now() < deadline {
            self.assert_running();

            match fetch_text_response(listen_addr, host, path) {
                Ok((status, body)) if status == expected_status && body == expected_body => {
                    return;
                }
                Ok((status, body)) => {
                    last_error = format!(
                        "unexpected response from {listen_addr} host={host:?} path={path:?}: status={status} body={body:?}"
                    );
                }
                Err(error) => last_error = error,
            }

            thread::sleep(Duration::from_millis(50));
        }

        panic!(
            "timed out waiting for response on {} host={:?} path={:?}; expected status={} body={:?}; last error: {}",
            listen_addr, host, path, expected_status, expected_body, last_error
        );
    }

    fn shutdown_and_wait(&mut self, timeout: Duration) {
        self.child.kill().expect("rginx should accept a kill signal");
        let status = self.wait_for_exit(timeout);
        assert!(
            !status.success() || status.code() == Some(0),
            "rginx should exit after the test, got {status}"
        );
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;

        loop {
            if let Some(status) = self.child.try_wait().expect("child status should be readable") {
                return status;
            }

            if Instant::now() >= deadline {
                let _ = self.child.kill();
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
            let _ = self.child.kill();
            let _ = self.child.wait();
        }

        let _ = fs::remove_dir_all(&self.temp_dir);
    }
}

fn fetch_text_response(
    listen_addr: SocketAddr,
    host: &str,
    path: &str,
) -> Result<(u16, String), String> {
    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;

    write!(stream, "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")
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

fn vhost_config(listen_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"default.example.com\"],\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Static(\n                status: Some(200),\n                content_type: Some(\"text/plain; charset=utf-8\"),\n                body: \"default root\\n\",\n            ),\n        ),\n    ],\n    servers: [\n        VirtualHostConfig(\n            server_names: [\"api.example.com\"],\n            locations: [\n                LocationConfig(\n                    matcher: Exact(\"/\"),\n                    handler: Static(\n                        status: Some(200),\n                        content_type: Some(\"text/plain; charset=utf-8\"),\n                        body: \"api root\\n\",\n                    ),\n                ),\n                LocationConfig(\n                    matcher: Exact(\"/users\"),\n                    handler: Static(\n                        status: Some(200),\n                        content_type: Some(\"text/plain; charset=utf-8\"),\n                        body: \"api users\\n\",\n                    ),\n                ),\n            ],\n        ),\n        VirtualHostConfig(\n            server_names: [\"*.internal.example.com\"],\n            locations: [\n                LocationConfig(\n                    matcher: Exact(\"/\"),\n                    handler: Static(\n                        status: Some(200),\n                        content_type: Some(\"text/plain; charset=utf-8\"),\n                        body: \"internal root\\n\",\n                    ),\n                ),\n            ],\n        ),\n    ],\n)\n",
        listen_addr.to_string()
    )
}

fn no_fallback_config(listen_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"default.example.com\"],\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/users\"),\n            handler: Static(\n                status: Some(200),\n                content_type: Some(\"text/plain; charset=utf-8\"),\n                body: \"default users\\n\",\n            ),\n        ),\n    ],\n    servers: [\n        VirtualHostConfig(\n            server_names: [\"api.example.com\"],\n            locations: [\n                LocationConfig(\n                    matcher: Exact(\"/status\"),\n                    handler: Static(\n                        status: Some(200),\n                        content_type: Some(\"text/plain; charset=utf-8\"),\n                        body: \"api status\\n\",\n                    ),\n                ),\n            ],\n        ),\n    ],\n)\n",
        listen_addr.to_string()
    )
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
