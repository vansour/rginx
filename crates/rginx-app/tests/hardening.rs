#![cfg(unix)]

use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, ExitStatus};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[test]
fn header_read_timeout_closes_slow_request_connections() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(
        listen_addr,
        static_config(listen_addr, Some("header_read_timeout_secs: Some(1),"), "ok\n"),
    );

    server.wait_for_body(listen_addr, "/", "ok\n", Duration::from_secs(5));

    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .expect("slow client should connect");
    stream.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
    stream.set_write_timeout(Some(Duration::from_millis(500))).unwrap();
    write!(stream, "GET / HTTP/1.1\r\nHost: {listen_addr}\r\n").unwrap();
    stream.flush().unwrap();

    thread::sleep(Duration::from_millis(1_500));

    assert_connection_closed(&mut stream, Some(b"Connection: close\r\n\r\n"));
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn max_connections_rejects_new_connections_when_limit_is_reached() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(
        listen_addr,
        static_config(listen_addr, Some("max_connections: Some(1),"), "ok\n"),
    );

    server.wait_for_body(listen_addr, "/", "ok\n", Duration::from_secs(5));

    let mut held = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .expect("first client should connect");
    held.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
    held.set_write_timeout(Some(Duration::from_millis(500))).unwrap();
    write!(held, "GET / HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: keep-alive\r\n\r\n")
        .unwrap();
    held.flush().unwrap();

    let response = read_http_response_once(&mut held).expect("first connection should succeed");
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.ends_with("ok\n"));

    let mut extra = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .expect("second client should connect before being rejected");
    extra.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
    extra.set_write_timeout(Some(Duration::from_millis(500))).unwrap();

    assert_connection_closed(&mut extra, Some(request_bytes(listen_addr, "/").as_bytes()));
    drop(held);
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn keep_alive_disabled_closes_connections_after_each_response() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(
        listen_addr,
        static_config(listen_addr, Some("keep_alive: Some(false),"), "ok\n"),
    );

    server.wait_for_body(listen_addr, "/", "ok\n", Duration::from_secs(5));

    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .expect("client should connect");
    stream.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
    stream.set_write_timeout(Some(Duration::from_millis(500))).unwrap();
    write!(stream, "GET / HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: keep-alive\r\n\r\n")
        .unwrap();
    stream.flush().unwrap();

    let response = read_http_response_once(&mut stream).expect("first response should be readable");
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.ends_with("ok\n"));

    assert_connection_closed(&mut stream, Some(request_bytes(listen_addr, "/").as_bytes()));
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn max_headers_rejects_requests_with_too_many_headers() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(
        listen_addr,
        static_config(listen_addr, Some("max_headers: Some(2),"), "ok\n"),
    );

    server.wait_for_body(listen_addr, "/", "ok\n", Duration::from_secs(5));

    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .expect("client should connect");
    stream.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
    stream.set_write_timeout(Some(Duration::from_millis(500))).unwrap();
    write!(
        stream,
        "GET / HTTP/1.1\r\nHost: {listen_addr}\r\nX-Test: 1\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    stream.flush().unwrap();

    let response = read_http_response_once(&mut stream).expect("response should be readable");
    assert!(
        response.starts_with("HTTP/1.1 431"),
        "expected 431 for header overflow, got {response:?}"
    );
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn max_request_body_bytes_rejects_chunked_proxy_requests_that_exceed_limit() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let upstream_addr = spawn_response_server("backend ok\n");
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(
        listen_addr,
        proxy_config(listen_addr, upstream_addr, Some("max_request_body_bytes: Some(8),")),
    );

    server.wait_for_body(listen_addr, "/api/ready", "backend ok\n", Duration::from_secs(5));

    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .expect("client should connect");
    stream.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
    stream.set_write_timeout(Some(Duration::from_millis(500))).unwrap();
    write!(
        stream,
        "POST /api/upload HTTP/1.1\r\nHost: {listen_addr}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n5\r\nworld\r\n0\r\n\r\n"
    )
    .unwrap();
    stream.flush().unwrap();

    let response = read_http_response_once(&mut stream).expect("response should be readable");
    let (status, _body) = parse_response(&response).expect("response should be valid HTTP");
    assert_eq!(status, 413, "expected 413 for oversized chunked body, got {response:?}");
    server.shutdown_and_wait(Duration::from_secs(5));
}

struct TestServer {
    child: Child,
    temp_dir: PathBuf,
}

impl TestServer {
    fn spawn(listen_addr: SocketAddr, config: String) -> Self {
        let temp_dir = temp_dir("rginx-hardening-test");
        fs::create_dir_all(&temp_dir).expect("temp test dir should be created");
        let config_path = temp_dir.join("rginx.ron");
        fs::write(&config_path, config).expect("config should be written");

        let child = std::process::Command::new(binary_path())
            .arg("--config")
            .arg(&config_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("rginx should start");

        let _ = listen_addr;
        Self { child, temp_dir }
    }

    fn wait_for_body(
        &mut self,
        listen_addr: SocketAddr,
        path: &str,
        expected: &str,
        timeout: Duration,
    ) {
        let deadline = Instant::now() + timeout;
        let mut last_error = String::new();

        while Instant::now() < deadline {
            self.assert_running();

            match fetch_text_response(listen_addr, path) {
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

fn static_config(listen_addr: SocketAddr, server_extra: Option<&str>, body: &str) -> String {
    let extra = server_extra.map(|value| format!("\n        {value}")).unwrap_or_default();
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},{}\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Static(\n                status: Some(200),\n                content_type: Some(\"text/plain; charset=utf-8\"),\n                body: {:?},\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        extra,
        body
    )
}

fn proxy_config(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    server_extra: Option<&str>,
) -> String {
    let extra = server_extra.map(|value| format!("\n        {value}")).unwrap_or_default();
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},{}\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            request_timeout_secs: Some(2),\n            unhealthy_after_failures: Some(2),\n            unhealthy_cooldown_secs: Some(1),\n        ),\n    ],\n    locations: [\n        LocationConfig(\n            matcher: Prefix(\"/api\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        extra,
        format!("http://{upstream_addr}")
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

    write!(stream, "{}", request_bytes(listen_addr, path))
        .map_err(|error| format!("failed to write request: {error}"))?;
    stream.flush().map_err(|error| format!("failed to flush request: {error}"))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| format!("failed to read response: {error}"))?;

    parse_response(&response)
}

fn request_bytes(listen_addr: SocketAddr, path: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n")
}

fn parse_response(response: &str) -> Result<(u16, String), String> {
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

fn read_http_response_once(stream: &mut TcpStream) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut response = Vec::new();

    while Instant::now() < deadline {
        let mut chunk = [0u8; 512];
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                response.extend_from_slice(&chunk[..read]);
                if response.windows(6).any(|window| window == b"\r\n\r\nok\n") {
                    break;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(format!("failed to read response: {error}")),
        }
    }

    String::from_utf8(response).map_err(|error| format!("invalid UTF-8 response: {error}"))
}

fn assert_connection_closed(stream: &mut TcpStream, trailing_bytes: Option<&[u8]>) {
    if let Some(bytes) = trailing_bytes {
        if stream.write_all(bytes).is_err() {
            return;
        }

        if stream.flush().is_err() {
            return;
        }
    }

    let mut buffer = [0u8; 64];
    match stream.read(&mut buffer) {
        Ok(0) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::UnexpectedEof
            ) => {}
        Ok(read) => panic!(
            "expected connection to be closed, received {:?}",
            String::from_utf8_lossy(&buffer[..read])
        ),
        Err(error) => panic!("expected connection to close cleanly, got {error}"),
    }
}

fn reserve_loopback_addr() -> SocketAddr {
    let listener =
        TcpListener::bind(("127.0.0.1", 0)).expect("ephemeral loopback listener should bind");
    let addr = listener.local_addr().expect("listener addr should be available");
    drop(listener);
    addr
}

fn spawn_response_server(body: &'static str) -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("test upstream listener should bind");
    let listen_addr = listener.local_addr().expect("listener addr should be available");

    thread::spawn(move || loop {
        let Ok((mut stream, _)) = listener.accept() else {
            break;
        };

        thread::spawn(move || {
            let mut buffer = [0u8; 1024];
            let _ = stream.read(&mut buffer);
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        });
    });

    listen_addr
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
