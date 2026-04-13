use super::*;

#[test]
fn status_command_reads_local_admin_socket() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-admin-status", |_| return_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));
    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let output = run_rginx(["--config", server.config_path().to_str().unwrap(), "status"]);
    assert!(output.status.success(), "status command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("kind=status"));
    assert!(stdout.contains("revision=0"));
    assert!(stdout.contains("listeners=1"));
    assert!(stdout.contains(&format!("listen_addrs={listen_addr}")));
    assert!(stdout.contains("kind=status_listener"));
    assert!(stdout.contains("listener=default"));
    assert!(stdout.contains(&format!("listen={listen_addr}")));
    assert!(stdout.contains("tls_listeners=1"));
    assert!(stdout.contains("tls_certificates=0"));
    assert!(stdout.contains("tls_expiring_certificates=0"));
    assert!(stdout.contains("http3_active_connections=0"));
    assert!(stdout.contains("http3_active_request_streams=0"));
    assert!(stdout.contains("http3_retry_issued_total=0"));
    assert!(stdout.contains("http3_request_body_stream_errors_total=0"));
    assert!(stdout.contains("active_connections=0"));
    assert!(stdout.contains("mtls_listeners=0"));
    assert!(stdout.contains("reload_attempts=0"));
    assert!(stdout.contains("last_reload=-"));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn status_command_reports_explicit_listener_inventory() {
    let http_addr = reserve_loopback_addr();
    let https_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-admin-status-explicit", |_| {
        explicit_listeners_config(&[("http", http_addr), ("https", https_addr)], "ok\n")
    });
    server.wait_for_http_ready(http_addr, Duration::from_secs(5));
    server.wait_for_http_text_response(
        https_addr,
        "example.com",
        "/",
        200,
        "ok\n",
        Duration::from_secs(5),
    );
    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let output = run_rginx(["--config", server.config_path().to_str().unwrap(), "status"]);
    assert!(output.status.success(), "status command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("listeners=2"));
    let status_line =
        stdout.lines().find(|line| line.contains("kind=status")).expect("status line should exist");
    let listen_addrs = status_line
        .split_whitespace()
        .find_map(|field| field.strip_prefix("listen_addrs="))
        .expect("listen_addrs field should exist");
    let mut actual = listen_addrs.split(',').map(str::to_string).collect::<Vec<_>>();
    actual.sort();
    let mut expected = vec![http_addr.to_string(), https_addr.to_string()];
    expected.sort();
    assert_eq!(actual, expected);
    assert!(stdout.contains("kind=status_listener"));
    assert!(stdout.contains("listener=http"));
    assert!(stdout.contains("listener=https"));
    assert!(stdout.contains(&format!("listen={http_addr}")));
    assert!(stdout.contains(&format!("listen={https_addr}")));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn status_command_reports_http3_listener_bindings() {
    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-admin-status-http3",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| {
            format!(
                "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n        accept_workers: Some(2),\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n            early_data: Some(true),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n            allow_early_data: Some(true),\n        ),\n    ],\n)\n",
                listen_addr.to_string(),
                cert_path.display().to_string(),
                key_path.display().to_string(),
                ready_route = READY_ROUTE_CONFIG,
            )
        },
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));
    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let output = run_rginx(["--config", server.config_path().to_str().unwrap(), "status"]);
    assert!(output.status.success(), "status command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("listener_bindings=2"));
    assert!(stdout.contains("bind_addrs=tcp://"));
    assert!(stdout.contains("udp://"));
    assert!(stdout.contains("http3=enabled"));
    assert!(stdout.contains("http3_early_data_enabled_listeners=1"));
    assert!(stdout.contains("http3_active_connections=0"));
    assert!(stdout.contains("http3_active_request_streams=0"));
    assert!(stdout.contains("http3_retry_issued_total=0"));
    assert!(stdout.contains("http3_request_accept_errors_total=0"));
    assert!(stdout.contains("http3_request_body_stream_errors_total=0"));
    assert!(stdout.contains("http3_response_stream_errors_total=0"));
    assert!(stdout.contains("http3_early_data_accepted_requests=0"));
    assert!(stdout.contains("http3_early_data_rejected_requests=0"));
    assert!(stdout.contains("kind=status_listener"));
    assert!(stdout.contains("transport_bindings=2"));
    assert!(stdout.contains("kind=status_listener_binding"));
    assert!(stdout.contains("binding=tcp"));
    assert!(stdout.contains("binding=udp"));
    assert!(stdout.contains("transport=udp"));
    assert!(stdout.contains("protocols=http3"));
    assert!(stdout.contains("worker_count=2"));
    assert!(stdout.contains("reuse_port_enabled=true"));
    assert!(stdout.contains("advertise_alt_svc=true"));
    assert!(stdout.contains("alt_svc_max_age_secs=7200"));
    assert!(stdout.contains("http3_max_concurrent_streams=128"));
    assert!(stdout.contains("http3_stream_buffer_size=65536"));
    assert!(stdout.contains("http3_active_connection_id_limit=2"));
    assert!(stdout.contains("http3_retry=false"));
    assert!(stdout.contains("http3_host_key_path=-"));
    assert!(stdout.contains("http3_gso=false"));
    assert!(stdout.contains("http3_early_data_enabled=true"));
    assert!(stdout.contains("kind=status_listener_http3"));
    assert!(stdout.contains("retry_issued_total=0"));
    assert!(stdout.contains("request_accept_errors_total=0"));
    assert!(stdout.contains("request_body_stream_errors_total=0"));
    assert!(stdout.contains("kind=status_tls_listener"));
    assert!(stdout.contains("http3_enabled=true"));
    assert!(stdout.contains("http3_versions=TLS1.3"));
    assert!(stdout.contains("http3_alpn_protocols=h3"));
    assert!(stdout.contains("http3_max_concurrent_streams=128"));
    assert!(stdout.contains("http3_stream_buffer_size=65536"));
    assert!(stdout.contains("http3_active_connection_id_limit=2"));
    assert!(stdout.contains("http3_retry=false"));
    assert!(stdout.contains("http3_host_key_path=-"));
    assert!(stdout.contains("http3_gso=false"));
    assert!(stdout.contains("http3_early_data_enabled=true"));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn counters_command_reports_local_connection_and_response_counters() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-admin-counters", |_| return_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));
    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    fetch_text_response(listen_addr, "/").expect("root request should succeed");
    fetch_text_response(listen_addr, "/missing").expect("missing request should respond");

    let output = run_rginx(["--config", server.config_path().to_str().unwrap(), "counters"]);
    assert!(output.status.success(), "counters command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("kind=counters"));
    assert!(stdout.contains("downstream_mtls_authenticated_requests_total=0"));
    assert!(stdout.contains("downstream_http3_early_data_accepted_requests_total=0"));
    assert!(stdout.contains("downstream_http3_early_data_rejected_requests_total=0"));
    let requests = parse_counter(&stdout, "downstream_requests_total");
    let responses_2xx = parse_counter(&stdout, "downstream_responses_2xx_total");
    let responses_4xx = parse_counter(&stdout, "downstream_responses_4xx_total");
    assert!(requests >= 3, "expected at least three requests, got {requests}: {stdout}");
    assert!(
        responses_2xx >= 2,
        "expected at least two 2xx responses, got {responses_2xx}: {stdout}"
    );
    assert!(
        responses_4xx >= 1,
        "expected at least one 4xx response, got {responses_4xx}: {stdout}"
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn traffic_command_reports_listener_vhost_and_route_counters() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-admin-traffic", |_| return_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));
    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let (status, body) =
        fetch_text_response(listen_addr, "/").expect("root request should succeed");
    assert_eq!(status, 200);
    assert_eq!(body, "ok\n");
    let (status, body) =
        fetch_text_response(listen_addr, "/missing").expect("missing request should respond");
    assert_eq!(status, 404);
    assert_eq!(body, "route not found\n");

    let response =
        query_admin_socket(&socket_path, AdminRequest::GetTrafficStats { window_secs: Some(300) })
            .expect("admin socket should return traffic stats");
    let AdminResponse::TrafficStats(traffic) = response else {
        panic!("admin socket should return traffic stats");
    };
    assert_eq!(traffic.listeners.len(), 1);
    assert_eq!(traffic.listeners[0].listener_id, "default");
    assert!(traffic.listeners[0].downstream_requests >= 3);
    assert!(traffic.listeners[0].unmatched_requests_total >= 1);
    assert!(traffic.listeners[0].downstream_responses_2xx >= 2);
    assert!(traffic.listeners[0].downstream_responses_4xx >= 1);
    assert_eq!(traffic.vhosts.len(), 1);
    assert_eq!(traffic.vhosts[0].vhost_id, "server");
    assert!(traffic.vhosts[0].downstream_requests >= 3);
    assert!(traffic.vhosts[0].unmatched_requests_total >= 1);
    let route = traffic
        .routes
        .iter()
        .find(|route| route.route_id.ends_with("|exact:/"))
        .expect("root route should be present in traffic stats");
    assert_eq!(route.vhost_id, "server");
    assert_eq!(route.downstream_requests, 1);
    assert_eq!(route.downstream_responses_2xx, 1);
    assert_eq!(route.recent_60s.window_secs, 60);
    assert_eq!(route.recent_60s.downstream_requests_total, 1);
    assert_eq!(route.recent_60s.downstream_responses_total, 1);
    assert_eq!(route.recent_window.as_ref().map(|recent| recent.window_secs), Some(300));
    assert_eq!(
        route.recent_window.as_ref().map(|recent| recent.downstream_requests_total),
        Some(1)
    );

    let output = run_rginx([
        "--config",
        server.config_path().to_str().unwrap(),
        "traffic",
        "--window-secs",
        "300",
    ]);
    assert!(output.status.success(), "traffic command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("kind=traffic_listener"));
    assert!(stdout.contains("kind=traffic_vhost"));
    assert!(stdout.contains("kind=traffic_route"));
    assert!(stdout.contains("kind=traffic_listener_recent_window"));
    assert!(stdout.contains("kind=traffic_route_recent_window"));
    assert!(stdout.contains("listener=default"));
    assert!(stdout.contains("vhost=server"));
    assert!(stdout.contains("route=server/routes"));
    assert!(stdout.contains("unmatched_requests_total=1"));
    assert!(stdout.contains("downstream_requests_total=1"));
    assert!(stdout.contains("recent_60s_window_secs=60"));
    assert!(stdout.contains("recent_60s_downstream_requests_total=1"));
    assert!(stdout.contains("recent_window_secs=300"));
    assert!(stdout.contains("recent_window_downstream_requests_total=1"));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn traffic_command_reports_grpc_request_and_status_counters() {
    let listen_addr = reserve_loopback_addr();
    let mut server =
        ServerHarness::spawn("rginx-admin-traffic-grpc", |_| return_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));
    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = send_raw_request(
        listen_addr,
        &format!(
            "POST /grpc.health.v1.Health/Check HTTP/1.1\r\nHost: {listen_addr}\r\nContent-Type: application/grpc\r\nTE: trailers\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ),
    )
    .expect("grpc-like request should succeed");
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.contains("grpc-status: 12"));

    let response =
        query_admin_socket(&socket_path, AdminRequest::GetTrafficStats { window_secs: None })
            .expect("admin socket should return traffic stats");
    let AdminResponse::TrafficStats(traffic) = response else {
        panic!("admin socket should return traffic stats");
    };
    assert_eq!(traffic.listeners.len(), 1);
    assert!(traffic.listeners[0].grpc.requests_total >= 1);
    assert!(traffic.listeners[0].grpc.protocol_grpc_total >= 1);
    assert!(traffic.listeners[0].grpc.status_12_total >= 1);
    assert!(traffic.vhosts[0].grpc.requests_total >= 1);
    assert!(traffic.vhosts[0].grpc.status_12_total >= 1);

    let output = run_rginx(["--config", server.config_path().to_str().unwrap(), "traffic"]);
    assert!(output.status.success(), "traffic command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("kind=traffic_listener"));
    assert!(stdout.contains("grpc_requests_total=1"));
    assert!(stdout.contains("grpc_protocol_grpc_total=1"));
    assert!(stdout.contains("grpc_status_12_total=1"));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn peers_command_reports_upstream_health_snapshot() {
    let listen_addr = reserve_loopback_addr();
    let upstream_addr = reserve_loopback_addr();
    let mut server =
        ServerHarness::spawn("rginx-admin-peers", |_| proxy_config(listen_addr, upstream_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));
    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let output = run_rginx(["--config", server.config_path().to_str().unwrap(), "peers"]);
    assert!(output.status.success(), "peers command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("kind=peer_health_upstream"));
    assert!(stdout.contains("kind=peer_health_peer"));
    assert!(stdout.contains("upstream=backend"));
    assert!(stdout.contains(&format!("peer=http://{upstream_addr}")));
    assert!(stdout.contains("available=true"));
    assert!(stdout.contains("backup=false"));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn upstreams_command_reports_upstream_request_counters() {
    let listen_addr = reserve_loopback_addr();
    let upstream_addr = spawn_response_server("admin upstream ok\n");
    let mut server =
        ServerHarness::spawn("rginx-admin-upstreams", |_| proxy_config(listen_addr, upstream_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));
    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let (status, body) =
        fetch_text_response(listen_addr, "/api/demo").expect("proxy request should succeed");
    assert_eq!(status, 200);
    assert_eq!(body, "admin upstream ok\n");

    let response =
        query_admin_socket(&socket_path, AdminRequest::GetUpstreamStats { window_secs: Some(300) })
            .expect("admin socket should return upstream stats");
    let AdminResponse::UpstreamStats(upstreams) = response else {
        panic!("admin socket should return upstream stats");
    };
    assert_eq!(upstreams.len(), 1);
    assert_eq!(upstreams[0].upstream_name, "backend");
    assert_eq!(upstreams[0].downstream_requests_total, 1);
    assert_eq!(upstreams[0].peer_attempts_total, 1);
    assert_eq!(upstreams[0].peer_successes_total, 1);
    assert_eq!(upstreams[0].peer_failures_total, 0);
    assert_eq!(upstreams[0].peer_timeouts_total, 0);
    assert_eq!(upstreams[0].failovers_total, 0);
    assert_eq!(upstreams[0].completed_responses_total, 1);
    assert_eq!(upstreams[0].bad_gateway_responses_total, 0);
    assert_eq!(upstreams[0].gateway_timeout_responses_total, 0);
    assert_eq!(upstreams[0].bad_request_responses_total, 0);
    assert_eq!(upstreams[0].payload_too_large_responses_total, 0);
    assert_eq!(upstreams[0].unsupported_media_type_responses_total, 0);
    assert_eq!(upstreams[0].no_healthy_peers_total, 0);
    assert_eq!(upstreams[0].recent_60s.window_secs, 60);
    assert_eq!(upstreams[0].recent_60s.downstream_requests_total, 1);
    assert_eq!(upstreams[0].recent_60s.peer_attempts_total, 1);
    assert_eq!(upstreams[0].recent_60s.completed_responses_total, 1);
    assert_eq!(upstreams[0].recent_window.as_ref().map(|recent| recent.window_secs), Some(300));
    assert_eq!(
        upstreams[0].recent_window.as_ref().map(|recent| recent.downstream_requests_total),
        Some(1)
    );
    assert_eq!(upstreams[0].peers.len(), 1);
    assert_eq!(upstreams[0].peers[0].peer_url, format!("http://{upstream_addr}"));
    assert_eq!(upstreams[0].peers[0].attempts_total, 1);
    assert_eq!(upstreams[0].peers[0].successes_total, 1);

    let output = run_rginx([
        "--config",
        server.config_path().to_str().unwrap(),
        "upstreams",
        "--window-secs",
        "300",
    ]);
    assert!(
        output.status.success(),
        "upstreams command should succeed: {}",
        render_output(&output)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("kind=upstream_stats"));
    assert!(stdout.contains("kind=upstream_stats_peer"));
    assert!(stdout.contains("kind=upstream_stats_recent_window"));
    assert!(stdout.contains("upstream=backend"));
    assert!(stdout.contains("downstream_requests_total=1"));
    assert!(stdout.contains("peer_attempts_total=1"));
    assert!(stdout.contains("peer_successes_total=1"));
    assert!(stdout.contains("completed_responses_total=1"));
    assert!(stdout.contains("recent_60s_window_secs=60"));
    assert!(stdout.contains("recent_60s_downstream_requests_total=1"));
    assert!(stdout.contains("recent_window_secs=300"));
    assert!(stdout.contains("recent_window_downstream_requests_total=1"));
    assert!(stdout.contains(&format!("peer=http://{upstream_addr}")));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn snapshot_and_delta_reflect_listener_lifecycle_changes_after_reload() {
    let http_addr = reserve_loopback_addr();
    let admin_listener_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-admin-listener-lifecycle", |_| {
        explicit_listeners_config(&[("http", http_addr)], "before reload\n")
    });
    server.wait_for_http_ready(http_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = query_admin_socket(&socket_path, AdminRequest::GetSnapshotVersion)
        .expect("admin socket should return snapshot version");
    let AdminResponse::SnapshotVersion(snapshot) = response else {
        panic!("admin socket should return snapshot version");
    };
    let since_version = snapshot.snapshot_version;

    std::fs::write(
        server.config_path(),
        explicit_listeners_config(
            &[("http", http_addr), ("admin", admin_listener_addr)],
            "after reload\n",
        ),
    )
    .expect("reloaded config should be written");
    server.send_signal(libc::SIGHUP);

    server.wait_for_http_text_response(
        admin_listener_addr,
        &admin_listener_addr.to_string(),
        "/",
        200,
        "after reload\n",
        Duration::from_secs(5),
    );

    let response = query_admin_socket(
        &socket_path,
        AdminRequest::GetSnapshot {
            include: Some(vec![rginx_http::SnapshotModule::Traffic]),
            window_secs: None,
        },
    )
    .expect("admin socket should return traffic snapshot");
    let AdminResponse::Snapshot(snapshot) = response else {
        panic!("admin socket should return traffic snapshot");
    };
    let traffic = snapshot.traffic.expect("traffic snapshot should be included");
    assert_eq!(traffic.listeners.len(), 2);
    assert!(traffic.listeners.iter().any(|listener| listener.listener_id == "listener:http"));
    assert!(traffic.listeners.iter().any(|listener| listener.listener_id == "listener:admin"));

    let response = query_admin_socket(
        &socket_path,
        AdminRequest::GetDelta {
            since_version,
            include: Some(vec![
                rginx_http::SnapshotModule::Status,
                rginx_http::SnapshotModule::Traffic,
            ]),
            window_secs: None,
        },
    )
    .expect("admin socket should return delta");
    let AdminResponse::Delta(delta) = response else {
        panic!("admin socket should return delta");
    };
    assert_eq!(delta.status_changed, Some(true));
    assert_eq!(delta.traffic_changed, Some(true));
    assert!(
        delta
            .changed_listener_ids
            .as_ref()
            .is_some_and(|listeners| listeners.iter().any(|listener| listener == "listener:admin"))
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}
