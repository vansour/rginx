use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[test]
fn idempotent_requests_fail_over_after_upstream_timeout() {
    let slow_hits = Arc::new(AtomicUsize::new(0));
    let fast_hits = Arc::new(AtomicUsize::new(0));
    let slow_peer = spawn_response_server_with_hits(
        Duration::from_millis(1_500),
        "slow peer\n",
        slow_hits.clone(),
    );
    let fast_peer =
        spawn_response_server_with_hits(Duration::from_millis(0), "fast peer\n", fast_hits.clone());
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-failover-test", |_| {
        proxy_config(listen_addr, &[slow_peer, fast_peer])
    });

    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));
    let (status, body) =
        fetch_text_response(listen_addr, "/api/demo").expect("failover request should succeed");
    assert_eq!(status, 200);
    assert_eq!(body, "fast peer\n");
    assert!(slow_hits.load(Ordering::SeqCst) > 0, "slow peer should be attempted first");
    assert!(fast_hits.load(Ordering::SeqCst) > 0, "fast peer should receive the failover attempt");
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn non_idempotent_requests_do_not_fail_over_after_upstream_timeout() {
    let slow_hits = Arc::new(AtomicUsize::new(0));
    let fast_hits = Arc::new(AtomicUsize::new(0));
    let slow_peer = spawn_response_server_with_hits(
        Duration::from_millis(1_500),
        "slow peer\n",
        slow_hits.clone(),
    );
    let fast_peer =
        spawn_response_server_with_hits(Duration::from_millis(0), "fast peer\n", fast_hits.clone());
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-failover-non-idempotent-test", |_| {
        proxy_config(listen_addr, &[slow_peer, fast_peer])
    });

    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));
    let (status, body) = send_text_request(
        listen_addr,
        "POST",
        "/api/demo",
        Some("payload"),
        Duration::from_millis(2_500),
    )
    .expect("non-idempotent request should return a terminal response");
    assert_eq!(status, 504);
    assert!(
        body.contains("timed out after 1000 ms"),
        "expected upstream timeout body, got {body:?}"
    );
    assert!(slow_hits.load(Ordering::SeqCst) > 0, "slow peer should be attempted before timeout");
    assert_eq!(fast_hits.load(Ordering::SeqCst), 0, "POST requests must not fail over");

    server.shutdown_and_wait(Duration::from_secs(5));
}

fn spawn_response_server_with_hits(
    delay: Duration,
    body: &'static str,
    hits: Arc<AtomicUsize>,
) -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("test upstream listener should bind");
    let listen_addr = listener.local_addr().expect("listener addr should be available");

    thread::spawn(move || {
        loop {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let hits = hits.clone();

            thread::spawn(move || {
                hits.fetch_add(1, Ordering::Relaxed);
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);
                thread::sleep(delay);

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

fn fetch_text_response(listen_addr: SocketAddr, path: &str) -> Result<(u16, String), String> {
    send_text_request(listen_addr, "GET", path, None, Duration::from_millis(2_500))
}

fn send_text_request(
    listen_addr: SocketAddr,
    method: &str,
    path: &str,
    body: Option<&str>,
    read_timeout: Duration,
) -> Result<(u16, String), String> {
    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(read_timeout))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;

    match body {
        Some(body) => write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .map_err(|error| format!("failed to write request: {error}"))?,
        None => write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n"
        )
        .map_err(|error| format!("failed to write request: {error}"))?,
    }
    stream.flush().map_err(|error| format!("failed to flush request: {error}"))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| format!("failed to read response: {error}"))?;

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

fn proxy_config(listen_addr: SocketAddr, upstreams: &[SocketAddr]) -> String {
    let peers = upstreams
        .iter()
        .map(|addr| {
            format!(
                "                UpstreamPeerConfig(\n                    url: {:?},\n                )",
                format!("http://{addr}")
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");

    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n{}\n            ],\n            request_timeout_secs: Some(1),\n            unhealthy_after_failures: Some(2),\n            unhealthy_cooldown_secs: Some(1),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/api\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        peers,
        ready_route = READY_ROUTE_CONFIG,
    )
}
