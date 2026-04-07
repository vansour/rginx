use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, read_http_head, reserve_loopback_addr};

#[test]
fn active_health_checks_mark_peer_unhealthy_and_recover_after_successive_probes() {
    let health_ok = Arc::new(AtomicBool::new(false));
    let upstream_addr = spawn_active_health_upstream(health_ok.clone());
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-active-health", |_| {
        active_health_config(listen_addr, upstream_addr)
    });

    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    wait_for_proxy_response(
        listen_addr,
        "GET",
        "/api/demo",
        502,
        "upstream `backend` has no healthy peers available",
        Duration::from_secs(6),
        "peer should enter cooldown after failed probes",
        &mut server,
    );

    health_ok.store(true, Ordering::Relaxed);

    server.wait_for_http_text_response(
        listen_addr,
        &listen_addr.to_string(),
        "/api/demo",
        200,
        "backend ok\n",
        Duration::from_secs(5),
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

fn wait_for_proxy_response(
    listen_addr: SocketAddr,
    method: &str,
    path: &str,
    expected_status: u16,
    expected_body_contains: &str,
    timeout: Duration,
    expectation: &str,
    server: &mut ServerHarness,
) {
    let deadline = std::time::Instant::now() + timeout;
    let mut last_error = String::new();

    while std::time::Instant::now() < deadline {
        server.assert_running();

        match send_http_request(listen_addr, method, path) {
            Ok(response) => {
                if response.status == expected_status
                    && String::from_utf8_lossy(&response.body).contains(expected_body_contains)
                {
                    return;
                }
                last_error = format!(
                    "unexpected response: status={} body={:?}",
                    response.status,
                    String::from_utf8_lossy(&response.body)
                );
            }
            Err(error) => last_error = error,
        }

        thread::sleep(Duration::from_millis(100));
    }

    panic!("{expectation}; last_error={last_error}\n{}", server.combined_output());
}

fn spawn_active_health_upstream(health_ok: Arc<AtomicBool>) -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("upstream listener should bind");
    let listen_addr = listener.local_addr().expect("upstream listener addr should be available");

    thread::spawn(move || {
        loop {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let health_ok = health_ok.clone();

            thread::spawn(move || {
                let head = read_http_head(&mut stream);
                let path = request_path(&head);
                let (status, body) = if path == "/healthz" {
                    if health_ok.load(Ordering::Relaxed) {
                        ("200 OK", "healthy\n")
                    } else {
                        ("503 Service Unavailable", "unhealthy\n")
                    }
                } else {
                    ("200 OK", "backend ok\n")
                };

                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            });
        }
    });

    listen_addr
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

fn request_path(head: &str) -> &str {
    head.lines().next().and_then(|line| line.split_whitespace().nth(1)).unwrap_or("/")
}

fn active_health_config(listen_addr: SocketAddr, upstream_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            request_timeout_secs: Some(1),\n            health_check_path: Some(\"/healthz\"),\n            health_check_interval_secs: Some(1),\n            health_check_timeout_secs: Some(1),\n            healthy_successes_required: Some(2),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/api\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("http://{upstream_addr}"),
        ready_route = READY_ROUTE_CONFIG,
    )
}
