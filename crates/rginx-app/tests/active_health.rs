use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[test]
fn active_health_checks_mark_peer_unhealthy_and_recover_after_successive_probes() {
    let health_ok = Arc::new(AtomicBool::new(false));
    let upstream_addr = spawn_active_health_upstream(health_ok.clone());
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-active-health", |_| {
        active_health_config(listen_addr, upstream_addr)
    });

    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    wait_for_status(
        &mut server,
        listen_addr,
        Duration::from_secs(5),
        "peer should become actively unhealthy after failed probes",
        |status| peer_status(status)["active_unhealthy"].as_bool() == Some(true),
    );

    let response =
        send_http_request(listen_addr, "GET", "/api/demo").expect("proxy request should respond");
    assert_eq!(response.status, 502);
    assert!(
        String::from_utf8_lossy(&response.body)
            .contains("upstream `backend` has no healthy peers available"),
        "unexpected proxy failure body: {:?}",
        String::from_utf8_lossy(&response.body)
    );

    let metrics = metrics_body(
        send_http_request(listen_addr, "GET", "/metrics").expect("metrics request should succeed"),
    );
    assert_metric_at_least(
        &metrics,
        &format!(
            "rginx_active_health_checks_total{{upstream=\"backend\",peer=\"http://{upstream_addr}\",result=\"unhealthy_status\"}}"
        ),
        1,
    );

    health_ok.store(true, Ordering::Relaxed);

    wait_for_status(
        &mut server,
        listen_addr,
        Duration::from_secs(3),
        "peer should require one successful probe before recovery",
        |status| {
            let peer = peer_status(status);
            peer["active_unhealthy"].as_bool() == Some(true)
                && peer["active_consecutive_successes"].as_u64() == Some(1)
        },
    );

    wait_for_status(
        &mut server,
        listen_addr,
        Duration::from_secs(5),
        "peer should recover after the configured number of successful probes",
        |status| {
            let peer = peer_status(status);
            peer["active_unhealthy"].as_bool() == Some(false)
                && peer["healthy"].as_bool() == Some(true)
        },
    );

    server.wait_for_http_text_response(
        listen_addr,
        &listen_addr.to_string(),
        "/api/demo",
        200,
        "backend ok\n",
        Duration::from_secs(5),
    );

    let metrics = metrics_body(
        send_http_request(listen_addr, "GET", "/metrics").expect("metrics request should succeed"),
    );
    assert_metric_at_least(
        &metrics,
        &format!(
            "rginx_active_health_checks_total{{upstream=\"backend\",peer=\"http://{upstream_addr}\",result=\"healthy\"}}"
        ),
        2,
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

fn peer_status(status: &Value) -> &Value {
    &status["upstreams"][0]["peers"][0]
}

fn wait_for_status(
    server: &mut ServerHarness,
    listen_addr: SocketAddr,
    timeout: Duration,
    expectation: &str,
    predicate: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + timeout;
    let mut last_status = Value::Null;
    let mut last_error = String::new();

    while Instant::now() < deadline {
        server.assert_running();

        match send_http_request(listen_addr, "GET", "/status") {
            Ok(response) if response.status == 200 => {
                match serde_json::from_slice::<Value>(&response.body) {
                    Ok(status) => {
                        if predicate(&status) {
                            return status;
                        }
                        last_status = status;
                    }
                    Err(error) => last_error = format!("status body should be JSON: {error}"),
                }
            }
            Ok(response) => {
                last_error = format!(
                    "unexpected /status response: status={} body={:?}",
                    response.status,
                    String::from_utf8_lossy(&response.body)
                );
            }
            Err(error) => last_error = error,
        }

        thread::sleep(Duration::from_millis(50));
    }

    panic!(
        "{expectation}; last_status={last_status}; last_error={last_error}\n{}",
        server.combined_output()
    );
}

fn metrics_body(response: ParsedResponse) -> String {
    assert_eq!(response.status, 200, "metrics endpoint should return 200");
    String::from_utf8(response.body).expect("metrics body should be valid UTF-8")
}

fn assert_metric_at_least(metrics: &str, prefix: &str, min_value: u64) {
    let value = metrics
        .lines()
        .find_map(|line| {
            let (metric, value) = line.split_once(' ')?;
            if metric == prefix { value.parse::<u64>().ok() } else { None }
        })
        .unwrap_or_else(|| panic!("missing metric line with prefix `{prefix}` in:\n{metrics}"));
    assert!(
        value >= min_value,
        "expected metric `{prefix}` >= {min_value}, got {value}\n{metrics}"
    );
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
                let Ok(head) = read_http_head(&mut stream) else {
                    return;
                };
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

fn read_http_head(stream: &mut TcpStream) -> Result<String, String> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 256];

    loop {
        let read = stream.read(&mut chunk).map_err(|error| format!("read failed: {error}"))?;
        if read == 0 {
            return Err("stream closed before request head completed".to_string());
        }
        buffer.extend_from_slice(&chunk[..read]);

        if let Some(head_end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            return String::from_utf8(buffer[..head_end + 4].to_vec())
                .map_err(|error| format!("request head should be utf-8: {error}"));
        }
    }
}

fn request_path(head: &str) -> &str {
    head.lines().next().and_then(|line| line.split_whitespace().nth(1)).unwrap_or("/")
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

fn active_health_config(listen_addr: SocketAddr, upstream_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            request_timeout_secs: Some(1),\n            health_check_path: Some(\"/healthz\"),\n            health_check_interval_secs: Some(1),\n            health_check_timeout_secs: Some(1),\n            healthy_successes_required: Some(2),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/status\"),\n            handler: Status,\n        ),\n        LocationConfig(\n            matcher: Exact(\"/metrics\"),\n            handler: Metrics,\n        ),\n        LocationConfig(\n            matcher: Prefix(\"/api\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("http://{upstream_addr}"),
        ready_route = READY_ROUTE_CONFIG,
    )
}
