use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[test]
fn ip_hash_keeps_clients_sticky_and_spreads_across_peers() {
    let peer_a = spawn_response_server("peer-a\n");
    let peer_b = spawn_response_server("peer-b\n");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-ip-hash-test", |_| {
        proxy_config(listen_addr, &[peer_a, peer_b])
    });

    let sticky_ip = "198.51.100.10";
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));
    let first = fetch_text_response(listen_addr, "/api/demo", sticky_ip)
        .expect("sticky request should succeed");
    let second = fetch_text_response(listen_addr, "/api/demo", sticky_ip)
        .expect("second sticky request should succeed");
    let third = fetch_text_response(listen_addr, "/api/demo", sticky_ip)
        .expect("third sticky request should succeed");

    assert_eq!(first.0, 200);
    assert_eq!(second.0, 200);
    assert_eq!(third.0, 200);
    assert_eq!(first.1, second.1);
    assert_eq!(second.1, third.1);

    let unique_bodies = (1..=16)
        .map(|suffix| {
            fetch_text_response(listen_addr, "/api/demo", &format!("198.51.100.{suffix}"))
                .expect("hashed request should succeed")
                .1
        })
        .collect::<HashSet<_>>();

    assert!(
        unique_bodies.contains("peer-a\n") && unique_bodies.contains("peer-b\n"),
        "expected ip_hash to distribute requests across both peers, got {unique_bodies:?}"
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

fn fetch_text_response(
    listen_addr: SocketAddr,
    path: &str,
    forwarded_for: &str,
) -> Result<(u16, String), String> {
    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;

    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {listen_addr}\r\nX-Forwarded-For: {forwarded_for}\r\nConnection: close\r\n\r\n"
    )
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
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        trusted_proxies: [\"127.0.0.1/32\"],\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n{}\n            ],\n            load_balance: IpHash,\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/api\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        peers,
        ready_route = READY_ROUTE_CONFIG,
    )
}
