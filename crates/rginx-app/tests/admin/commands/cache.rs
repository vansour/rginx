use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::super::*;

#[test]
fn cache_command_reports_inventory_and_purge_cache_key_removes_entry() {
    let listen_addr = reserve_loopback_addr();
    let (upstream_addr, upstream_hits) = spawn_counting_response_server("admin cache ok\n");
    let mut server = ServerHarness::spawn("rginx-admin-cache", |temp_dir| {
        cached_proxy_config(listen_addr, upstream_addr, &temp_dir.join("cache"))
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let first = send_raw_request(
        listen_addr,
        &format!(
            "GET /api/demo HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n"
        ),
    )
    .expect("first request should succeed");
    assert_eq!(response_header_value(&first, "x-cache").as_deref(), Some("MISS"));
    assert_eq!(upstream_hits.load(Ordering::Relaxed), 1);

    let second = send_raw_request(
        listen_addr,
        &format!(
            "GET /api/demo HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n"
        ),
    )
    .expect("second request should succeed");
    assert_eq!(response_header_value(&second, "x-cache").as_deref(), Some("HIT"));
    assert_eq!(upstream_hits.load(Ordering::Relaxed), 1);

    let output = run_rginx(["--config", server.config_path().to_str().unwrap(), "cache"]);
    assert!(output.status.success(), "cache command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("kind=cache_summary"));
    assert!(stdout.contains("zones=1"));
    assert!(stdout.contains("entries=1"));
    assert!(stdout.contains("hit_total=1"));
    assert!(stdout.contains("miss_total=1"));
    assert!(stdout.contains("kind=cache_zone"));
    assert!(stdout.contains("zone=default"));

    let key = format!("http:{listen_addr}:/api/demo");
    let output = run_rginx([
        "--config",
        server.config_path().to_str().unwrap(),
        "purge-cache",
        "--zone",
        "default",
        "--key",
        &key,
    ]);
    assert!(
        output.status.success(),
        "purge-cache command should succeed: {}",
        render_output(&output)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("kind=cache_purge"));
    assert!(stdout.contains("removed_entries=1"));

    let third = send_raw_request(
        listen_addr,
        &format!(
            "GET /api/demo HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n"
        ),
    )
    .expect("third request should succeed");
    assert_eq!(response_header_value(&third, "x-cache").as_deref(), Some("MISS"));
    assert_eq!(upstream_hits.load(Ordering::Relaxed), 2);

    server.shutdown_and_wait(Duration::from_secs(5));
}

fn cached_proxy_config(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    cache_dir: &Path,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    cache_zones: [\n        CacheZoneConfig(\n            name: \"default\",\n            path: {:?},\n            max_size_bytes: Some(1048576),\n            inactive_secs: Some(600),\n            default_ttl_secs: Some(60),\n            max_entry_bytes: Some(65536),\n        ),\n    ],\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/api/demo\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n            cache: Some(CacheRouteConfig(\n                zone: \"default\",\n                methods: Some([\"GET\", \"HEAD\"]),\n                statuses: Some([200]),\n                key: Some(\"{{scheme}}:{{host}}:{{uri}}\"),\n                stale_if_error_secs: Some(60),\n            )),\n        ),\n    ],\n)\n",
        cache_dir.display().to_string(),
        listen_addr.to_string(),
        format!("http://{upstream_addr}"),
        ready_route = READY_ROUTE_CONFIG,
    )
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

fn response_header_value(response: &str, header_name: &str) -> Option<String> {
    let (head, _) = response.split_once("\r\n\r\n")?;
    head.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case(header_name).then(|| value.trim().to_string())
    })
}
