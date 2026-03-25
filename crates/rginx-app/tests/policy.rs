use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[test]
fn route_access_control_allows_and_denies_requests_end_to_end() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-policy-acl", |_| acl_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    server.wait_for_http_text_response(
        listen_addr,
        &listen_addr.to_string(),
        "/allow",
        200,
        "allowed\n",
        Duration::from_secs(5),
    );

    let response =
        send_http_request(listen_addr, "GET", "/deny").expect("deny route should respond");
    assert_eq!(response.status, 403);
    assert_eq!(response.body, b"forbidden\n");

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn route_rate_limit_rejects_requests_after_capacity_is_exhausted() {
    let listen_addr = reserve_loopback_addr();
    let mut server =
        ServerHarness::spawn("rginx-policy-rate-limit", |_| rate_limit_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    server.wait_for_http_text_response(
        listen_addr,
        &listen_addr.to_string(),
        "/limited",
        200,
        "limited ok\n",
        Duration::from_secs(5),
    );

    let response = send_http_request(listen_addr, "GET", "/limited")
        .expect("rate-limited route should respond");
    assert_eq!(response.status, 429);
    assert_eq!(response.body, b"hold your horses! too many requests\n");

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[derive(Debug)]
struct ParsedResponse {
    status: u16,
    body: Vec<u8>,
}

fn send_http_request(
    listen_addr: SocketAddr,
    method: &str,
    path: &str,
) -> Result<ParsedResponse, String> {
    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;

    write!(stream, "{method} {path} HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n")
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

    Ok(ParsedResponse { status, body: bytes[head_end + 4..].to_vec() })
}

fn acl_config(listen_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/allow\"),\n            handler: Static(\n                status: Some(200),\n                content_type: Some(\"text/plain; charset=utf-8\"),\n                body: \"allowed\\n\",\n            ),\n            allow_cidrs: [\"127.0.0.1/32\", \"::1/128\"],\n        ),\n        LocationConfig(\n            matcher: Exact(\"/deny\"),\n            handler: Static(\n                status: Some(200),\n                content_type: Some(\"text/plain; charset=utf-8\"),\n                body: \"should not be returned\\n\",\n            ),\n            deny_cidrs: [\"127.0.0.1/32\", \"::1/128\"],\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn rate_limit_config(listen_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/limited\"),\n            handler: Static(\n                status: Some(200),\n                content_type: Some(\"text/plain; charset=utf-8\"),\n                body: \"limited ok\\n\",\n            ),\n            requests_per_sec: Some(1),\n            burst: Some(0),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}
