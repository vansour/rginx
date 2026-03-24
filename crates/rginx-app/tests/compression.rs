use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use brotli::Decompressor;
use flate2::read::GzDecoder;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[test]
fn gzips_large_static_text_responses_when_client_accepts_gzip() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-compression-static", |_| {
        static_config(listen_addr, &"hello gzip world\n".repeat(32))
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let response = send_http_request(
        listen_addr,
        &format!(
            "GET / HTTP/1.1\r\nHost: {listen_addr}\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n"
        ),
    )
    .expect("gzip request should succeed");

    assert_eq!(response.status, 200);
    assert_eq!(response.header("content-encoding"), Some("gzip"));
    assert_eq!(response.header("vary"), Some("Accept-Encoding"));
    assert_eq!(decode_gzip(&response.body), "hello gzip world\n".repeat(32).into_bytes());

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn brotlis_large_static_text_responses_when_client_accepts_brotli() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-compression-static-br", |_| {
        static_config(listen_addr, &"hello brotli world\n".repeat(32))
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let response = send_http_request(
        listen_addr,
        &format!(
            "GET / HTTP/1.1\r\nHost: {listen_addr}\r\nAccept-Encoding: br, gzip;q=0.5\r\nConnection: close\r\n\r\n"
        ),
    )
    .expect("brotli request should succeed");

    assert_eq!(response.status, 200);
    assert_eq!(response.header("content-encoding"), Some("br"));
    assert_eq!(response.header("vary"), Some("Accept-Encoding"));
    assert_eq!(decode_brotli(&response.body), "hello brotli world\n".repeat(32).into_bytes());

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn falls_back_to_gzip_when_client_prefers_it_over_brotli() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-compression-static-gzip-fallback", |_| {
        static_config(listen_addr, &"hello gzip fallback\n".repeat(32))
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let response = send_http_request(
        listen_addr,
        &format!(
            "GET / HTTP/1.1\r\nHost: {listen_addr}\r\nAccept-Encoding: br;q=0.2, gzip;q=0.8\r\nConnection: close\r\n\r\n"
        ),
    )
    .expect("gzip fallback request should succeed");

    assert_eq!(response.status, 200);
    assert_eq!(response.header("content-encoding"), Some("gzip"));
    assert_eq!(decode_gzip(&response.body), "hello gzip fallback\n".repeat(32).into_bytes());

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn skips_gzip_for_range_file_responses() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-compression-file", |temp_dir| {
        let root = temp_dir.join("public");
        fs::create_dir_all(&root).expect("file root should be created");
        fs::write(root.join("hello.txt"), b"0123456789abcdef0123456789abcdef")
            .expect("test file should be written");
        file_config(listen_addr, &root)
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let response = send_http_request(
        listen_addr,
        &format!(
            "GET /hello.txt HTTP/1.1\r\nHost: {listen_addr}\r\nAccept-Encoding: gzip\r\nRange: bytes=2-5\r\nConnection: close\r\n\r\n"
        ),
    )
    .expect("range request should succeed");

    assert_eq!(response.status, 206);
    assert_eq!(response.body, b"2345");
    assert_eq!(response.header("content-encoding"), None);
    assert_eq!(response.header("content-range"), Some("bytes 2-5/32"));

    server.shutdown_and_wait(Duration::from_secs(5));
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
    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
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

fn decode_gzip(bytes: &[u8]) -> Vec<u8> {
    let mut decoder = GzDecoder::new(bytes);
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded).expect("gzip body should decode");
    decoded
}

fn decode_brotli(bytes: &[u8]) -> Vec<u8> {
    let mut decoder = Decompressor::new(bytes, 4096);
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded).expect("brotli body should decode");
    decoded
}

fn static_config(listen_addr: SocketAddr, body: &str) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Static(\n                status: Some(200),\n                content_type: Some(\"text/plain; charset=utf-8\"),\n                body: {:?},\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        body,
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn file_config(listen_addr: SocketAddr, root: &Path) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/\"),\n            handler: File(\n                root: {:?},\n                index: None,\n                try_files: Some([\"$uri\"]),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        root,
        ready_route = READY_ROUTE_CONFIG,
    )
}
