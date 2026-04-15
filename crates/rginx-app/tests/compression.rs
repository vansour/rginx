use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use brotli::Decompressor;
use flate2::read::GzDecoder;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[test]
fn gzips_large_return_text_responses_when_client_accepts_gzip() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-compression-return", |_| {
        return_config(listen_addr, &"hello gzip world\n".repeat(32))
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
fn brotlis_large_return_text_responses_when_client_accepts_brotli() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-compression-return-br", |_| {
        return_config(listen_addr, &"hello brotli world\n".repeat(32))
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
    let mut server = ServerHarness::spawn("rginx-compression-return-gzip-fallback", |_| {
        return_config(listen_addr, &"hello gzip fallback\n".repeat(32))
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
fn skips_compression_when_response_buffering_is_off() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-compression-buffering-off", |_| {
        return_config_with_route_options(
            listen_addr,
            &"hello passthrough world\n".repeat(32),
            "            response_buffering: Some(Off),\n",
        )
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let response = send_http_request(
        listen_addr,
        &format!(
            "GET / HTTP/1.1\r\nHost: {listen_addr}\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n"
        ),
    )
    .expect("request should succeed");

    assert_eq!(response.status, 200);
    assert_eq!(response.header("content-encoding"), None);
    assert_eq!(response.body, "hello passthrough world\n".repeat(32).into_bytes());

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn force_compression_can_encode_small_responses_when_response_buffering_is_on() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-compression-force-small", |_| {
        return_config_with_route_options(
            listen_addr,
            &"a".repeat(128),
            "            response_buffering: Some(On),\n            compression: Some(Force),\n",
        )
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let response = send_http_request(
        listen_addr,
        &format!(
            "GET / HTTP/1.1\r\nHost: {listen_addr}\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n"
        ),
    )
    .expect("force compression request should succeed");

    assert_eq!(response.status, 200);
    assert_eq!(response.header("content-encoding"), Some("gzip"));
    assert_eq!(decode_gzip(&response.body), "a".repeat(128).into_bytes());

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn custom_compression_content_types_can_disable_default_text_compression() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-compression-content-types", |_| {
        return_config_with_route_options(
            listen_addr,
            &"hello allowlist world\n".repeat(32),
            "            response_buffering: Some(On),\n            compression_content_types: Some([\"application/json\"]),\n",
        )
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let response = send_http_request(
        listen_addr,
        &format!(
            "GET / HTTP/1.1\r\nHost: {listen_addr}\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n"
        ),
    )
    .expect("allowlist request should succeed");

    assert_eq!(response.status, 200);
    assert_eq!(response.header("content-encoding"), None);
    assert_eq!(response.body, "hello allowlist world\n".repeat(32).into_bytes());

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

fn return_config(listen_addr: SocketAddr, body: &str) -> String {
    return_config_with_route_options(listen_addr, body, "")
}

fn return_config_with_route_options(
    listen_addr: SocketAddr,
    body: &str,
    route_options: &str,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some({:?}),\n            ),\n{route_options}        ),\n    ],\n)\n",
        listen_addr.to_string(),
        body,
        ready_route = READY_ROUTE_CONFIG,
        route_options = route_options,
    )
}
