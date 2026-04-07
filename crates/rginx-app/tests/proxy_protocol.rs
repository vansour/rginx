#![cfg(unix)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[test]
fn trusted_proxy_protocol_and_xff_preserve_client_chain() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-proxy-protocol", |_| return_config(listen_addr));
    wait_for_proxy_protocol_ready(listen_addr, Duration::from_secs(5));

    let response = send_proxy_protocol_request(
        listen_addr,
        "PROXY TCP4 203.0.113.10 127.0.0.1 12345 80\r\n",
        "GET / HTTP/1.1\r\nHost: example.com\r\nX-Forwarded-For: 198.51.100.9\r\nConnection: close\r\n\r\n",
    )
    .expect("proxy protocol request should succeed");
    assert!(response.starts_with("HTTP/1.1 200"), "unexpected response: {response:?}");

    server.shutdown_and_wait(Duration::from_secs(5));
    let logs = server.combined_output();
    assert!(
        logs.contains("remote=198.51.100.9 peer=127.0.0.1"),
        "expected access log to contain resolved client chain, got {logs:?}"
    );
    assert!(logs.contains("source=x_forwarded_for"), "expected XFF source in logs: {logs:?}");
}

#[test]
fn untrusted_transport_peer_ignores_proxy_protocol_source() {
    let listen_addr = reserve_loopback_addr();
    let mut server =
        ServerHarness::spawn("rginx-proxy-protocol-untrusted", |_| untrusted_config(listen_addr));
    wait_for_proxy_protocol_ready(listen_addr, Duration::from_secs(5));

    let response = send_proxy_protocol_request(
        listen_addr,
        "PROXY TCP4 198.51.100.9 127.0.0.1 12345 80\r\n",
        "GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n",
    )
    .expect("proxy protocol request should succeed");
    assert!(response.starts_with("HTTP/1.1 200"), "unexpected response: {response:?}");

    server.shutdown_and_wait(Duration::from_secs(5));
    let logs = server.combined_output();
    assert!(
        logs.contains("remote=127.0.0.1"),
        "expected socket peer to remain client ip: {logs:?}"
    );
    assert!(logs.contains("source=socket_peer"), "expected socket peer source in logs: {logs:?}");
}

fn wait_for_proxy_protocol_ready(listen_addr: SocketAddr, timeout: Duration) {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if send_proxy_protocol_request(
            listen_addr,
            "PROXY TCP4 127.0.0.1 127.0.0.1 12345 80\r\n",
            "GET /-/ready HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n",
        )
        .is_ok()
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    panic!("timed out waiting for proxy protocol listener on {listen_addr}");
}

fn send_proxy_protocol_request(
    listen_addr: SocketAddr,
    proxy_line: &str,
    request: &str,
) -> Result<String, String> {
    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;
    stream
        .write_all(proxy_line.as_bytes())
        .map_err(|error| format!("failed to write proxy line: {error}"))?;
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

fn return_config(listen_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        proxy_protocol: Some(true),\n        trusted_proxies: [\"127.0.0.1/32\", \"203.0.113.0/24\"],\n        access_log_format: Some(\"ACCESS remote=$remote_addr peer=$peer_addr source=$client_ip_source\"),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn untrusted_config(listen_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        proxy_protocol: Some(true),\n        trusted_proxies: [],\n        access_log_format: Some(\"ACCESS remote=$remote_addr peer=$peer_addr source=$client_ip_source\"),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}
