use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[test]
fn custom_access_log_format_emits_expected_line() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-access-log", |_| static_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let response = send_http_request(
        listen_addr,
        &format!(
            "GET / HTTP/1.1\r\nHost: {listen_addr}\r\nX-Request-ID: client-log-42\r\nUser-Agent: access-log-test/1.0\r\nConnection: close\r\n\r\n"
        ),
    )
    .expect("request should succeed");
    assert!(response.starts_with("HTTP/1.1 200"));

    server.shutdown_and_wait(Duration::from_secs(5));
    let logs = server.combined_output();
    assert!(
        logs.contains(
            "ACCESS reqid=client-log-42 status=200 request=\"GET / HTTP/1.1\" bytes=3 ua=\"access-log-test/1.0\""
        ),
        "expected access log line in stderr, got {logs:?}"
    );
}

fn static_config(listen_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        access_log_format: Some(\"ACCESS reqid=$request_id status=$status request=\\\"$request\\\" bytes=$body_bytes_sent ua=\\\"$http_user_agent\\\"\"),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Static(\n                status: Some(200),\n                content_type: Some(\"text/plain; charset=utf-8\"),\n                body: \"ok\\n\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn send_http_request(listen_addr: SocketAddr, request: &str) -> Result<String, String> {
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

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| format!("failed to read response: {error}"))?;
    Ok(response)
}
