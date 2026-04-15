use std::io::Write;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

mod support;

use support::{
    READY_ROUTE_CONFIG, ServerHarness, connect_http_client, read_http_chunk,
    read_http_head_and_pending, reserve_loopback_addr, spawn_scripted_chunked_response_server,
};

#[test]
fn proxy_streaming_download_delivers_first_chunk_before_upstream_completes() {
    let (upstream_addr, upstream_task) = spawn_scripted_chunked_response_server(
        "GET /stream HTTP/1.1\r\n",
        b"first\n",
        Duration::from_millis(900),
        Some(b"second\n"),
    );
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-streaming-download", |_| {
        proxy_streaming_config(listen_addr, upstream_addr, "")
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let mut client = connect_http_client(listen_addr, Duration::from_secs(3));
    write!(client, "GET /api/stream HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n")
        .expect("streaming request should write");
    client.flush().expect("streaming request should flush");

    let started = Instant::now();
    let (head, mut pending) = read_http_head_and_pending(&mut client);
    let head_lower = head.to_ascii_lowercase();
    assert!(head.starts_with("HTTP/1.1 200"), "unexpected response head: {head:?}");
    assert!(
        head_lower.contains("\r\ntransfer-encoding: chunked\r\n"),
        "streaming response should remain chunked, got {head:?}"
    );

    let first =
        read_http_chunk(&mut client, &mut pending).expect("first streaming chunk should arrive");
    assert_eq!(first, b"first\n");
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "first chunk should arrive before the upstream finishes streaming, elapsed={:?}",
        started.elapsed()
    );

    let second =
        read_http_chunk(&mut client, &mut pending).expect("second streaming chunk should arrive");
    assert_eq!(second, b"second\n");
    assert_eq!(
        read_http_chunk(&mut client, &mut pending),
        None,
        "stream should end after the scripted chunks"
    );

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.join().expect("upstream thread should complete");
}

#[test]
fn route_streaming_response_idle_timeout_closes_stalled_proxy_downloads() {
    let (upstream_addr, upstream_task) = spawn_scripted_chunked_response_server(
        "GET /stream HTTP/1.1\r\n",
        b"hello\n",
        Duration::from_secs(2),
        Some(b"late\n"),
    );
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-streaming-download-idle-timeout", |_| {
        proxy_streaming_config(
            listen_addr,
            upstream_addr,
            "            streaming_response_idle_timeout_secs: Some(1),\n",
        )
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let mut client = connect_http_client(listen_addr, Duration::from_secs(3));
    write!(client, "GET /api/stream HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n")
        .expect("streaming request should write");
    client.flush().expect("streaming request should flush");

    let (head, mut pending) = read_http_head_and_pending(&mut client);
    assert!(head.starts_with("HTTP/1.1 200"), "unexpected response head: {head:?}");
    let first = read_http_chunk(&mut client, &mut pending)
        .expect("first chunk should arrive before the stall");
    assert_eq!(first, b"hello\n");

    let stalled_at = Instant::now();
    let follow_up = read_http_chunk(&mut client, &mut pending);
    assert!(
        follow_up.is_none(),
        "stalled streaming response should terminate instead of delivering another chunk: {follow_up:?}"
    );
    assert!(
        stalled_at.elapsed() < Duration::from_millis(1800),
        "route idle timeout should cut off the stalled response before the upstream resumes, elapsed={:?}",
        stalled_at.elapsed()
    );

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.join().expect("upstream thread should complete");
}

fn proxy_streaming_config(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    route_extra: &str,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            request_timeout_secs: Some(5),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/api\"),\n            handler: Proxy(\n                upstream: \"backend\",\n                strip_prefix: Some(\"/api\"),\n            ),\n            response_buffering: Some(Off),\n            compression: Some(Off),\n{route_extra}        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("http://{upstream_addr}"),
        ready_route = READY_ROUTE_CONFIG,
        route_extra = route_extra,
    )
}
