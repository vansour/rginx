use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

mod support;

use support::{
    HttpChunkRead, READY_ROUTE_CONFIG, ServerHarness, connect_http_client, read_http_chunk,
    read_http_head_and_pending, reserve_loopback_addr, spawn_scripted_chunked_response_server,
};

#[test]
fn normalized_request_path_matches_dot_wildcard_vhost_on_real_server() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-nginx-alignment-vhost", |_| {
        normalized_vhost_config(listen_addr)
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let root = fetch_text_response(listen_addr, "example.com", "/api/../admin")
        .expect("root host request should succeed");
    assert_eq!(root, (200, "root admin\n".to_string()));

    let subdomain = fetch_text_response(listen_addr, "edge.example.com", "/api/../admin")
        .expect("subdomain request should succeed");
    assert_eq!(subdomain, (200, "root admin\n".to_string()));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn preferred_prefix_route_wins_against_regex_on_real_server() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-nginx-alignment-route", |_| {
        preferred_prefix_config(listen_addr)
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let response = fetch_text_response(listen_addr, &listen_addr.to_string(), "/assets/logo.svg")
        .expect("preferred prefix request should succeed");
    assert_eq!(response, (200, "preferred\n".to_string()));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn upstream_max_conns_fails_over_on_real_server() {
    let (slow_addr, slow_task) = spawn_scripted_chunked_response_server(
        "GET /stream HTTP/1.1\r\n",
        b"slow-first\n",
        Duration::from_millis(1_500),
        Some(b"slow-second\n"),
    );
    let fast_addr = spawn_response_server("fast peer\n");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-nginx-alignment-max-conns", |_| {
        proxy_max_conns_config(listen_addr, slow_addr, fast_addr)
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let mut client = connect_http_client(listen_addr, Duration::from_secs(3));
    write!(client, "GET /api/stream HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n")
        .expect("slow request should write");
    client.flush().expect("slow request should flush");

    let (head, mut pending) = read_http_head_and_pending(&mut client);
    assert!(head.starts_with("HTTP/1.1 200"), "unexpected response head: {head:?}");
    assert_eq!(
        read_http_chunk(&mut client, &mut pending),
        HttpChunkRead::Chunk(b"slow-first\n".to_vec())
    );

    let failover = fetch_text_response(listen_addr, &listen_addr.to_string(), "/api/stream")
        .expect("second request should succeed");
    assert_eq!(failover, (200, "fast peer\n".to_string()));

    assert_eq!(
        read_http_chunk(&mut client, &mut pending),
        HttpChunkRead::Chunk(b"slow-second\n".to_vec())
    );
    assert_eq!(read_http_chunk(&mut client, &mut pending), HttpChunkRead::End);

    server.shutdown_and_wait(Duration::from_secs(5));
    slow_task.join().expect("slow upstream thread should complete");
}

fn normalized_vhost_config(listen_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"default\\n\"),\n            ),\n        ),\n    ],\n    servers: [\n        VirtualHostConfig(\n            server_names: [\".example.com\"],\n            locations: [\n                LocationConfig(\n                    matcher: Exact(\"/admin\"),\n                    handler: Return(\n                        status: 200,\n                        location: \"\",\n                        body: Some(\"root admin\\n\"),\n                    ),\n                ),\n            ],\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn preferred_prefix_config(listen_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: PreferredPrefix(\"/assets\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"preferred\\n\"),\n            ),\n        ),\n        LocationConfig(\n            matcher: Regex(\n                pattern: \"^/assets/.*$\",\n                case_insensitive: false,\n            ),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"regex\\n\"),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn proxy_max_conns_config(
    listen_addr: SocketAddr,
    slow_addr: SocketAddr,
    fast_addr: SocketAddr,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            load_balance: IpHash,\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n                UpstreamPeerConfig(\n                    url: {:?},\n                    max_conns: Some(1),\n                ),\n            ],\n            request_timeout_secs: Some(5),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/api\"),\n            handler: Proxy(\n                upstream: \"backend\",\n                strip_prefix: Some(\"/api\"),\n            ),\n            response_buffering: Some(Off),\n            compression: Some(Off),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("http://{fast_addr}"),
        format!("http://{slow_addr}"),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn spawn_response_server(body: &'static str) -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("test upstream listener should bind");
    let listen_addr = listener.local_addr().expect("listener addr should be available");

    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };

        let mut buffer = [0u8; 1024];
        let _ = stream.read(&mut buffer);
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    });

    listen_addr
}

fn fetch_text_response(
    listen_addr: SocketAddr,
    host: &str,
    path: &str,
) -> Result<(u16, String), String> {
    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")
        .map_err(|error| format!("failed to write request: {error}"))?;
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
