use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[test]
fn least_conn_prefers_the_peer_with_fewer_in_flight_requests() {
    let (slow_tx, slow_rx) = mpsc::channel();
    let slow_peer = spawn_response_server(Duration::from_millis(600), "slow-peer\n", Some(slow_tx));
    let fast_peer = spawn_response_server(Duration::from_millis(0), "fast-peer\n", None);
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-least-conn-test", |_| {
        proxy_config(listen_addr, &[slow_peer, fast_peer])
    });

    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));
    while slow_rx.try_recv().is_ok() {}

    let first_request = thread::spawn(move || {
        fetch_text_response(listen_addr, "/api/hold")
            .expect("first least_conn request should eventually succeed")
    });

    slow_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("first request should hit the slow peer first");

    let second = fetch_text_response(listen_addr, "/api/hold")
        .expect("second least_conn request should succeed");
    assert_eq!(second.0, 200);
    assert_eq!(second.1, "fast-peer\n");

    let first = first_request.join().expect("first request thread should finish");
    assert_eq!(first.0, 200);
    assert_eq!(first.1, "slow-peer\n");

    server.shutdown_and_wait(Duration::from_secs(5));
}

fn spawn_response_server(
    delay: Duration,
    body: &'static str,
    accepted_signal: Option<mpsc::Sender<()>>,
) -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("test upstream listener should bind");
    let listen_addr = listener.local_addr().expect("listener addr should be available");

    thread::spawn(move || {
        loop {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            if let Some(signal) = accepted_signal.as_ref() {
                let _ = signal.send(());
            }

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
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n{}\n            ],\n            load_balance: LeastConn,\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/api\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        peers,
        ready_route = READY_ROUTE_CONFIG,
    )
}
