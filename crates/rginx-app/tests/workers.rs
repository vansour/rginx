use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[test]
fn serves_requests_with_configured_runtime_and_accept_workers() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-workers-test", |_| return_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let barrier = Arc::new(Barrier::new(9));
    let mut workers = Vec::new();
    for _ in 0..8 {
        let barrier = barrier.clone();
        workers.push(thread::spawn(move || {
            barrier.wait();
            send_http_request(
                listen_addr,
                &format!("GET / HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n"),
            )
            .expect("worker request should succeed")
        }));
    }

    barrier.wait();

    for worker in workers {
        let response = worker.join().expect("worker thread should finish");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"workers ok\n");
    }

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[derive(Debug)]
struct ParsedResponse {
    status: u16,
    body: Vec<u8>,
}

fn send_http_request(listen_addr: SocketAddr, request: &str) -> Result<ParsedResponse, String> {
    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(500))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;
    stream
        .write_all(request.as_bytes())
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

    for line in head.lines().skip(1) {
        let Some((_name, _value)) = line.split_once(':') else {
            return Err(format!("malformed header line: {line:?}"));
        };
    }

    Ok(ParsedResponse { status, body: bytes[head_end + 4..].to_vec() })
}

fn return_config(listen_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n        worker_threads: Some(2),\n        accept_workers: Some(2),\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"workers ok\\n\"),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        ready_route = READY_ROUTE_CONFIG
    )
}
