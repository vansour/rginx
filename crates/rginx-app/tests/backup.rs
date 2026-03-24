use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[test]
fn backup_peer_serves_requests_after_primary_timeout() {
    let slow_primary = spawn_response_server(Duration::from_millis(1_500), "primary slow\n");
    let fast_backup = spawn_response_server(Duration::from_millis(0), "backup fast\n");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-backup-test", |_| {
        proxy_config(listen_addr, slow_primary, fast_backup)
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let (status, body) =
        fetch_text_response(listen_addr, "/api/demo").expect("backup request should succeed");
    assert_eq!(status, 200);
    assert_eq!(body, "backup fast\n");
    server.shutdown_and_wait(Duration::from_secs(5));
}

fn spawn_response_server(delay: Duration, body: &'static str) -> SocketAddr {
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
    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(2_500)))
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

fn proxy_config(listen_addr: SocketAddr, primary: SocketAddr, backup: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                    weight: 1,\n                    backup: false,\n                ),\n                UpstreamPeerConfig(\n                    url: {:?},\n                    weight: 1,\n                    backup: true,\n                ),\n            ],\n            request_timeout_secs: Some(1),\n            unhealthy_after_failures: Some(1),\n            unhealthy_cooldown_secs: Some(30),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/api\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("http://{primary}"),
        format!("http://{backup}"),
        ready_route = READY_ROUTE_CONFIG,
    )
}
