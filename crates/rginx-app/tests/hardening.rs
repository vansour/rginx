#![cfg(unix)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[test]
fn header_read_timeout_closes_slow_request_connections() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(
        listen_addr,
        return_config(listen_addr, Some("header_read_timeout_secs: Some(1),"), "ok\n"),
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
        return_config(listen_addr, Some("max_connections: Some(1),"), "ok\n"),
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
        return_config(listen_addr, Some("keep_alive: Some(false),"), "ok\n"),
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
        return_config(listen_addr, Some("max_headers: Some(2),"), "ok\n"),
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

    server.wait_for_ready(listen_addr, Duration::from_secs(5));

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

#[test]
fn request_body_read_timeout_rejects_stalled_proxy_uploads() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let upstream_addr = spawn_drain_request_server();
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(
        listen_addr,
        proxy_config_with_upstream_extra(
            listen_addr,
            upstream_addr,
            Some("request_body_read_timeout_secs: Some(1),"),
            Some(
                "request_timeout_secs: Some(5),\n            unhealthy_after_failures: Some(2),\n            unhealthy_cooldown_secs: Some(1),",
            ),
        ),
    );

    server.wait_for_ready(listen_addr, Duration::from_secs(5));

    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .expect("client should connect");
    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    stream.set_write_timeout(Some(Duration::from_millis(500))).unwrap();
    write!(
        stream,
        "POST /api/upload HTTP/1.1\r\nHost: {listen_addr}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n"
    )
    .unwrap();
    stream.flush().unwrap();

    thread::sleep(Duration::from_millis(1_500));

    let response = read_http_response_bytes(&mut stream).expect("response should be readable");
    let parsed = parse_http_response(&response).expect("response should be valid HTTP");
    assert_eq!(parsed.status, 502, "expected 502 for timed out upload, got {parsed:?}");
    assert!(
        String::from_utf8_lossy(&parsed.body).contains("upstream `backend` is unavailable"),
        "unexpected timeout response body: {:?}",
        String::from_utf8_lossy(&parsed.body)
    );
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[derive(Debug)]
struct ParsedResponse {
    status: u16,
    body: Vec<u8>,
}

struct TestServer {
    inner: ServerHarness,
}

impl TestServer {
    fn spawn(listen_addr: SocketAddr, config: String) -> Self {
        let _ = listen_addr;
        Self { inner: ServerHarness::spawn("rginx-hardening-test", |_| config) }
    }

    fn wait_for_body(
        &mut self,
        listen_addr: SocketAddr,
        path: &str,
        expected: &str,
        timeout: Duration,
    ) {
        self.inner.wait_for_http_text_response(
            listen_addr,
            &listen_addr.to_string(),
            path,
            200,
            expected,
            timeout,
        );
    }

    fn wait_for_ready(&mut self, listen_addr: SocketAddr, timeout: Duration) {
        self.inner.wait_for_http_ready(listen_addr, timeout);
    }
    fn shutdown_and_wait(&mut self, timeout: Duration) {
        self.inner.terminate_and_wait(timeout);
    }
}

fn return_config(listen_addr: SocketAddr, server_extra: Option<&str>, body: &str) -> String {
    let extra = server_extra.map(|value| format!("\n        {value}")).unwrap_or_default();
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},{}\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some({:?}),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        extra,
        body,
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn proxy_config(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    server_extra: Option<&str>,
) -> String {
    proxy_config_with_upstream_extra(listen_addr, upstream_addr, server_extra, None)
}

fn proxy_config_with_upstream_extra(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    server_extra: Option<&str>,
    upstream_extra: Option<&str>,
) -> String {
    let extra = server_extra.map(|value| format!("\n        {value}")).unwrap_or_default();
    let upstream_extra = upstream_extra
        .unwrap_or(
            "request_timeout_secs: Some(2),\n            unhealthy_after_failures: Some(2),\n            unhealthy_cooldown_secs: Some(1),",
        )
        .to_string();
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},{}\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            {}\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/api\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        extra,
        format!("http://{upstream_addr}"),
        upstream_extra,
        ready_route = READY_ROUTE_CONFIG,
    )
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

fn read_http_response_bytes(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|error| format!("failed to read response bytes: {error}"))?;
    Ok(response)
}

fn parse_http_response(bytes: &[u8]) -> Result<ParsedResponse, String> {
    let head_end = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| format!("malformed response: {bytes:?}"))?;
    let head = String::from_utf8(bytes[..head_end].to_vec())
        .map_err(|error| format!("response head should be valid UTF-8: {error}"))?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| format!("missing status line: {head:?}"))?
        .parse::<u16>()
        .map_err(|error| format!("invalid status code: {error}"))?;

    for line in head.lines().skip(1) {
        if line.split_once(':').is_none() {
            return Err(format!("malformed header line: {line:?}"));
        }
    }

    Ok(ParsedResponse { status, body: bytes[head_end + 4..].to_vec() })
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

fn spawn_response_server(body: &'static str) -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("test upstream listener should bind");
    let listen_addr = listener.local_addr().expect("listener addr should be available");

    thread::spawn(move || {
        loop {
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
        }
    });

    listen_addr
}

fn spawn_drain_request_server() -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("test upstream listener should bind");
    let listen_addr = listener.local_addr().expect("listener addr should be available");

    thread::spawn(move || {
        loop {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };

            thread::spawn(move || {
                let mut buffer = [0u8; 4096];
                loop {
                    match stream.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
            });
        }
    });

    listen_addr
}

fn test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
