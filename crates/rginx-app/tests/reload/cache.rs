use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

#[test]
fn reload_preserves_cache_entries_when_zone_path_is_reused() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let (upstream_addr, upstream_hits) = spawn_counting_response_server("reload cache ok\n");
    let mut server = TestServer::spawn_with_setup("rginx-reload-cache", |temp_dir| {
        cached_proxy_config(listen_addr, upstream_addr, &temp_dir.join("cache"), 3)
    });

    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let first = send_raw_request(
        listen_addr,
        &format!("GET /api/demo HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n"),
    )
    .expect("first request should succeed");
    assert_eq!(response_header_value(&first, "x-cache").as_deref(), Some("MISS"));

    let second = send_raw_request(
        listen_addr,
        &format!("GET /api/demo HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n"),
    )
    .expect("second request should succeed");
    assert_eq!(response_header_value(&second, "x-cache").as_deref(), Some("HIT"));
    assert_eq!(upstream_hits.load(Ordering::Relaxed), 1);

    let cache_dir = server.temp_dir().join("cache");
    server.write_config(cached_proxy_config(listen_addr, upstream_addr, &cache_dir, 5));
    server.send_signal(libc::SIGHUP);
    server.wait_for_status_output(|output| output.contains("revision=1"), Duration::from_secs(5));

    let third = send_raw_request(
        listen_addr,
        &format!("GET /api/demo HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n"),
    )
    .expect("third request should succeed");
    assert_eq!(response_header_value(&third, "x-cache").as_deref(), Some("HIT"));
    assert_eq!(upstream_hits.load(Ordering::Relaxed), 1);

    server.shutdown_and_wait(Duration::from_secs(5));
}

fn cached_proxy_config(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    cache_dir: &Path,
    request_timeout_secs: u64,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    cache_zones: [\n        CacheZoneConfig(\n            name: \"default\",\n            path: {:?},\n            max_size_bytes: Some(1048576),\n            inactive_secs: Some(600),\n            default_ttl_secs: Some(60),\n            max_entry_bytes: Some(65536),\n        ),\n    ],\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            request_timeout_secs: Some({request_timeout_secs}),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/api/demo\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n            cache: Some(CacheRouteConfig(\n                zone: \"default\",\n                methods: Some([\"GET\", \"HEAD\"]),\n                statuses: Some([200]),\n                key: Some(\"{{scheme}}:{{host}}:{{uri}}\"),\n                stale_if_error_secs: Some(60),\n            )),\n        ),\n    ],\n)\n",
        cache_dir.display().to_string(),
        listen_addr.to_string(),
        format!("http://{upstream_addr}"),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn send_raw_request(listen_addr: SocketAddr, request: &str) -> Result<String, String> {
    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("failed to write request: {error}"))?;
    stream.flush().map_err(|error| format!("failed to flush request: {error}"))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| format!("failed to read response: {error}"))?;
    Ok(response)
}

fn response_header_value(response: &str, header_name: &str) -> Option<String> {
    let (head, _) = response.split_once("\r\n\r\n")?;
    head.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case(header_name).then(|| value.trim().to_string())
    })
}

fn spawn_counting_response_server(body: &'static str) -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("test upstream listener should bind");
    let listen_addr = listener.local_addr().expect("listener addr should be available");
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_task = hits.clone();

    std::thread::spawn(move || {
        loop {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let hits = hits_task.clone();
            std::thread::spawn(move || {
                hits.fetch_add(1, Ordering::Relaxed);
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncache-control: max-age=60\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            });
        }
    });

    (listen_addr, hits)
}
