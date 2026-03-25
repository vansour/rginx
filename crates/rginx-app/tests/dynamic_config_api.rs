use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use serde_json::Value;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[test]
fn config_api_reads_and_applies_updated_config() {
    let listen_addr = reserve_loopback_addr();
    let initial_config = dynamic_config_source(listen_addr, "before dynamic config\n");
    let mut server = ServerHarness::spawn("rginx-dynamic-config", |_| initial_config.clone());
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));
    server.wait_for_http_text_response(
        listen_addr,
        &listen_addr.to_string(),
        "/app",
        200,
        "before dynamic config\n",
        Duration::from_secs(5),
    );

    let response = send_http_request(listen_addr, "GET", "/-/config", None)
        .expect("config GET should succeed");
    assert_eq!(response.status, 200);
    let payload: Value =
        serde_json::from_slice(&response.body).expect("config GET should return JSON");
    assert_eq!(payload["revision"], 0);
    assert_eq!(payload["listen"], listen_addr.to_string());
    assert!(
        payload["config"]
            .as_str()
            .expect("config GET should include active config")
            .contains("before dynamic config\\n")
    );

    let updated_config = dynamic_config_source(listen_addr, "after dynamic config\n");
    let response =
        send_http_request(listen_addr, "PUT", "/-/config", Some(updated_config.as_bytes()))
            .expect("config PUT should succeed");
    assert_eq!(response.status, 200);
    let payload: Value =
        serde_json::from_slice(&response.body).expect("config PUT should return JSON");
    assert_eq!(payload["revision"], 1);
    assert!(payload["config"].is_null());

    server.wait_for_http_text_response(
        listen_addr,
        &listen_addr.to_string(),
        "/app",
        200,
        "after dynamic config\n",
        Duration::from_secs(5),
    );

    let response = send_http_request(listen_addr, "GET", "/-/config", None)
        .expect("config GET should succeed");
    let payload: Value =
        serde_json::from_slice(&response.body).expect("config GET should return JSON");
    assert_eq!(payload["revision"], 1);
    assert!(
        payload["config"]
            .as_str()
            .expect("config GET should include updated config")
            .contains("after dynamic config\\n")
    );
    assert_eq!(
        fs::read_to_string(server.config_path()).expect("config file should be readable"),
        updated_config
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn config_api_rejects_restart_required_changes() {
    let listen_addr = reserve_loopback_addr();
    let initial_config = dynamic_config_source(listen_addr, "stable dynamic config\n");
    let mut server =
        ServerHarness::spawn("rginx-dynamic-config-reject", |_| initial_config.clone());
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));
    server.wait_for_http_text_response(
        listen_addr,
        &listen_addr.to_string(),
        "/app",
        200,
        "stable dynamic config\n",
        Duration::from_secs(5),
    );

    let rejected_addr = reserve_loopback_addr();
    let rejected_config = dynamic_config_source(rejected_addr, "should not apply\n");
    let response =
        send_http_request(listen_addr, "PUT", "/-/config", Some(rejected_config.as_bytes()))
            .expect("rejected config PUT should respond");
    assert_eq!(response.status, 400);
    let payload: Value =
        serde_json::from_slice(&response.body).expect("rejected config PUT should return JSON");
    assert!(
        payload["error"]
            .as_str()
            .expect("error payload should contain a message")
            .contains("restart rginx instead")
    );

    server.wait_for_http_text_response(
        listen_addr,
        &listen_addr.to_string(),
        "/app",
        200,
        "stable dynamic config\n",
        Duration::from_secs(5),
    );
    assert_eq!(
        fs::read_to_string(server.config_path()).expect("config file should be readable"),
        initial_config
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn config_api_rejects_invalid_config_without_changing_revision_or_disk_state() {
    let listen_addr = reserve_loopback_addr();
    let initial_config = dynamic_config_source(listen_addr, "stable dynamic config\n");
    let mut server =
        ServerHarness::spawn("rginx-dynamic-config-invalid", |_| initial_config.clone());
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let invalid_config = dynamic_config_source(listen_addr, "broken dynamic config\n").replace(
        "            allow_cidrs: [\"127.0.0.1/32\", \"::1/128\"],\n",
        "            allow_cidrs: [],\n",
    );
    let response =
        send_http_request(listen_addr, "PUT", "/-/config", Some(invalid_config.as_bytes()))
            .expect("invalid config PUT should respond");
    assert_eq!(response.status, 400);
    let payload: Value =
        serde_json::from_slice(&response.body).expect("invalid config PUT should return JSON");
    assert!(
        payload["error"]
            .as_str()
            .expect("error payload should contain a message")
            .contains("requires non-empty allow_cidrs")
    );

    let response = send_http_request(listen_addr, "GET", "/-/config", None)
        .expect("config GET should succeed after rejected update");
    assert_eq!(response.status, 200);
    let payload: Value =
        serde_json::from_slice(&response.body).expect("config GET should return JSON");
    assert_eq!(payload["revision"], 0);
    assert!(
        payload["config"]
            .as_str()
            .expect("config GET should keep the original config")
            .contains("stable dynamic config\\n")
    );
    assert_eq!(
        fs::read_to_string(server.config_path()).expect("config file should stay unchanged"),
        initial_config
    );

    server.wait_for_http_text_response(
        listen_addr,
        &listen_addr.to_string(),
        "/app",
        200,
        "stable dynamic config\n",
        Duration::from_secs(5),
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn config_api_rejects_oversized_body_without_changing_revision_or_disk_state() {
    let listen_addr = reserve_loopback_addr();
    let initial_config = dynamic_config_source(listen_addr, "stable dynamic config\n");
    let mut server =
        ServerHarness::spawn("rginx-dynamic-config-oversized", |_| initial_config.clone());
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let oversized_body = vec![b'x'; 1024 * 1024 + 1];
    let response = send_http_request(listen_addr, "PUT", "/-/config", Some(&oversized_body))
        .expect("oversized config PUT should respond");
    assert_eq!(response.status, 413);
    let payload: Value =
        serde_json::from_slice(&response.body).expect("oversized config PUT should return JSON");
    assert!(
        payload["error"]
            .as_str()
            .expect("error payload should contain a message")
            .contains("exceeds 1048576 bytes")
    );

    let response = send_http_request(listen_addr, "GET", "/-/config", None)
        .expect("config GET should succeed after rejected oversized update");
    assert_eq!(response.status, 200);
    let payload: Value =
        serde_json::from_slice(&response.body).expect("config GET should return JSON");
    assert_eq!(payload["revision"], 0);
    assert_eq!(
        fs::read_to_string(server.config_path()).expect("config file should stay unchanged"),
        initial_config
    );

    server.wait_for_http_text_response(
        listen_addr,
        &listen_addr.to_string(),
        "/app",
        200,
        "stable dynamic config\n",
        Duration::from_secs(5),
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn config_api_rejects_unsupported_methods_with_allow_header() {
    let listen_addr = reserve_loopback_addr();
    let initial_config = dynamic_config_source(listen_addr, "stable dynamic config\n");
    let mut server =
        ServerHarness::spawn("rginx-dynamic-config-methods", |_| initial_config.clone());
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let response = send_http_request(listen_addr, "POST", "/-/config", None)
        .expect("unsupported config method should respond");
    assert_eq!(response.status, 405);
    assert_eq!(response.header("allow"), Some("GET, HEAD, PUT"));
    let payload: Value =
        serde_json::from_slice(&response.body).expect("method-not-allowed body should be JSON");
    assert_eq!(payload["error"], "method not allowed");

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[derive(Debug)]
struct ParsedResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl ParsedResponse {
    #[allow(dead_code)]
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
    }
}

fn send_http_request(
    listen_addr: SocketAddr,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> Result<ParsedResponse, String> {
    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;

    write!(stream, "{method} {path} HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n")
        .map_err(|error| format!("failed to write request head: {error}"))?;

    if let Some(body) = body {
        write!(
            stream,
            "Content-Type: application/ron; charset=utf-8\r\nContent-Length: {}\r\n",
            body.len()
        )
        .map_err(|error| format!("failed to write request headers: {error}"))?;
    }

    write!(stream, "\r\n").map_err(|error| format!("failed to finish request headers: {error}"))?;
    if let Some(body) = body {
        stream.write_all(body).map_err(|error| format!("failed to write request body: {error}"))?;
    }
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

fn dynamic_config_source(listen_addr: SocketAddr, app_body: &str) -> String {
    dynamic_config_source_with_listen(&listen_addr.to_string(), app_body)
}

fn dynamic_config_source_with_listen(listen: &str, app_body: &str) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/app\"),\n            handler: Static(\n                status: Some(200),\n                content_type: Some(\"text/plain; charset=utf-8\"),\n                body: {:?},\n            ),\n        ),\n        LocationConfig(\n            matcher: Exact(\"/-/config\"),\n            handler: Config,\n            allow_cidrs: [\"127.0.0.1/32\", \"::1/128\"],\n        ),\n    ],\n)\n",
        listen,
        app_body,
        ready_route = READY_ROUTE_CONFIG,
    )
}
