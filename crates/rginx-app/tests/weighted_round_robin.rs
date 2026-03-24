use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[test]
fn round_robin_honors_peer_weights() {
    let heavy_peer = spawn_response_server("heavy-peer\n");
    let light_peer = spawn_response_server("light-peer\n");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-weighted-round-robin-test", |_| {
        proxy_config(listen_addr, heavy_peer, light_peer)
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let observed = (0..8)
        .map(|_| {
            let (status, body) = fetch_text_response(listen_addr, "/api/demo")
                .expect("weighted request should work");
            assert_eq!(status, 200);
            body
        })
        .collect::<Vec<_>>();

    assert_eq!(
        observed,
        vec![
            "heavy-peer\n".to_string(),
            "heavy-peer\n".to_string(),
            "heavy-peer\n".to_string(),
            "light-peer\n".to_string(),
            "heavy-peer\n".to_string(),
            "heavy-peer\n".to_string(),
            "heavy-peer\n".to_string(),
            "light-peer\n".to_string(),
        ]
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

fn spawn_response_server(body: &'static str) -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("test upstream listener should bind");
    let listen_addr = listener.local_addr().expect("listener addr should be available");

    thread::spawn(move || {
        loop {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };

            thread::spawn(move || {
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
        }
    });

    listen_addr
}

fn fetch_text_response(listen_addr: SocketAddr, path: &str) -> Result<(u16, String), String> {
    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;

    write!(stream, "GET {path} HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n")
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

fn proxy_config(listen_addr: SocketAddr, heavy_peer: SocketAddr, light_peer: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                    weight: 3,\n                ),\n                UpstreamPeerConfig(\n                    url: {:?},\n                    weight: 1,\n                ),\n            ],\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/api\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("http://{heavy_peer}"),
        format!("http://{light_peer}"),
        ready_route = READY_ROUTE_CONFIG
    )
}
