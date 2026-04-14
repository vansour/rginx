use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, read_http_head, reserve_loopback_addr};

#[test]
fn return_responses_generate_and_preserve_request_id_headers() {
    let listen_addr = reserve_loopback_addr();
    let mut server =
        ServerHarness::spawn("rginx-phase1-return", |_| return_config(listen_addr, "ok\n"));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let head_response = send_http_request(
        listen_addr,
        &format!("HEAD / HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n"),
    )
    .expect("HEAD request should succeed");
    assert_eq!(head_response.status, 200);
    assert_eq!(head_response.body, b"");
    assert_eq!(head_response.header("content-length"), Some("3"));
    assert_generated_request_id(head_response.header("x-request-id"));

    let get_response = send_http_request(
        listen_addr,
        &format!(
            "GET / HTTP/1.1\r\nHost: {listen_addr}\r\nX-Request-ID: client-return-42\r\nConnection: close\r\n\r\n"
        ),
    )
    .expect("GET request should succeed");
    assert_eq!(get_response.status, 200);
    assert_eq!(get_response.body, b"ok\n");
    assert_eq!(get_response.header("x-request-id"), Some("client-return-42"));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn proxy_preserves_request_id_end_to_end() {
    let upstream_listener =
        TcpListener::bind(("127.0.0.1", 0)).expect("test upstream listener should bind");
    let upstream_addr =
        upstream_listener.local_addr().expect("upstream listener addr should be available");
    let upstream_task = thread::spawn(move || {
        let (mut stream, _) = upstream_listener.accept().expect("upstream should accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("upstream read timeout should be configurable");
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .expect("upstream write timeout should be configurable");

        let request = read_http_head(&mut stream);
        assert!(
            request.to_ascii_lowercase().contains("x-request-id: client-proxy-42\r\n"),
            "proxied request should preserve the incoming request id, got {request:?}"
        );

        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 11\r\nconnection: close\r\n\r\nbackend ok\n",
            )
            .expect("upstream should write a response");
        stream.flush().expect("upstream response should flush");
    });

    let listen_addr = reserve_loopback_addr();
    let mut server =
        ServerHarness::spawn("rginx-phase1-proxy", |_| proxy_config(listen_addr, upstream_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let response = send_http_request(
        listen_addr,
        &format!(
            "GET /api/demo HTTP/1.1\r\nHost: {listen_addr}\r\nX-Request-ID: client-proxy-42\r\nConnection: close\r\n\r\n"
        ),
    )
    .expect("proxy request should succeed");
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"backend ok\n");
    assert_eq!(response.header("x-request-id"), Some("client-proxy-42"));

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.join().expect("upstream thread should complete");
}

#[test]
fn proxy_request_buffering_off_streams_chunked_uploads_and_enforces_limits() {
    let upstream_listener =
        TcpListener::bind(("127.0.0.1", 0)).expect("test upstream listener should bind");
    let upstream_addr =
        upstream_listener.local_addr().expect("upstream listener addr should be available");
    let observed_request = Arc::new(Mutex::new(None));
    let observed_request_clone = observed_request.clone();
    let upstream_task = thread::spawn(move || {
        let (mut stream, _) = upstream_listener.accept().expect("upstream should accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("upstream read timeout should be configurable");
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .expect("upstream write timeout should be configurable");

        let (head, mut body) = read_http_head_and_body(&mut stream);
        let mut chunk = [0u8; 256];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => body.extend_from_slice(&chunk[..read]),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(error) => panic!("upstream should keep reading proxied body bytes: {error}"),
            }
        }

        *observed_request_clone.lock().expect("request observation lock should be available") =
            Some((head, body));
    });

    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-phase1-streaming-upload", |_| {
        proxy_streaming_limit_config(listen_addr, upstream_addr)
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let response = send_http_request(
        listen_addr,
        &format!(
            "POST /api/upload HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nabcde\r\n5\r\nfghij\r\n0\r\n\r\n"
        ),
    )
    .expect("chunked upload should return a response");

    assert_eq!(response.status, 413);
    assert!(
        String::from_utf8_lossy(&response.body).contains("max_request_body_bytes (8 bytes)"),
        "expected payload-too-large body, got {:?}",
        response.body
    );

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.join().expect("upstream thread should complete");

    let (head, body) = observed_request
        .lock()
        .expect("request observation lock should be available")
        .take()
        .expect("upstream should observe a streamed request");
    assert!(
        head.to_ascii_lowercase().contains("transfer-encoding: chunked\r\n"),
        "streamed upstream request should remain chunked, got {head:?}"
    );

    let body_text = String::from_utf8_lossy(&body);
    assert!(
        body_text.contains("abcde"),
        "upstream should receive the first chunk before the limit trips, got {body_text:?}"
    );
    assert!(
        !body_text.contains("fghij"),
        "upstream should not receive the over-limit chunk, got {body_text:?}"
    );
}

#[derive(Debug)]
struct ParsedResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl ParsedResponse {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
    }
}

fn send_http_request(listen_addr: SocketAddr, request: &str) -> Result<ParsedResponse, String> {
    send_http_request_with_timeouts(
        listen_addr,
        request,
        Duration::from_secs(2),
        Duration::from_millis(500),
    )
}

fn send_http_request_with_timeouts(
    listen_addr: SocketAddr,
    request: &str,
    read_timeout: Duration,
    write_timeout: Duration,
) -> Result<ParsedResponse, String> {
    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(read_timeout))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(write_timeout))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("failed to write request: {error}"))?;
    stream.flush().map_err(|error| format!("failed to flush request: {error}"))?;

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|error| format!("failed to read response: {error}"))?;
    parse_http_response(&response)
}

fn read_http_head_and_body(stream: &mut TcpStream) -> (String, Vec<u8>) {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 256];

    loop {
        let read = stream.read(&mut chunk).expect("HTTP head should be readable");
        assert!(read > 0, "stream closed before the HTTP head was complete");
        buffer.extend_from_slice(&chunk[..read]);

        if let Some(head_end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8(buffer[..head_end + 4].to_vec())
                .expect("HTTP head should be valid UTF-8");
            let body = buffer[head_end + 4..].to_vec();
            return (head, body);
        }
    }
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

    let mut headers = HashMap::new();
    for line in head.lines().skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            return Err(format!("malformed header line: {line:?}"));
        };
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }

    Ok(ParsedResponse { status, headers, body: bytes[head_end + 4..].to_vec() })
}

fn assert_generated_request_id(value: Option<&str>) {
    let value = value.expect("response should include x-request-id");
    assert_eq!(value.len(), "rginx-0000000000000000".len());
    assert!(value.starts_with("rginx-"), "generated request id should use the rginx- prefix");
    assert!(
        value["rginx-".len()..].chars().all(|ch| ch.is_ascii_hexdigit()),
        "generated request id should end with lowercase hex digits, got {value:?}"
    );
}

fn return_config(listen_addr: SocketAddr, body: &str) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some({:?}),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        body,
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn proxy_config(listen_addr: SocketAddr, upstream_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/api\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("http://{upstream_addr}"),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn proxy_streaming_limit_config(listen_addr: SocketAddr, upstream_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        max_request_body_bytes: Some(8),\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            request_timeout_secs: Some(1),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/api\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n            request_buffering: Some(Off),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("http://{upstream_addr}"),
        ready_route = READY_ROUTE_CONFIG,
    )
}
