use super::super::*;

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
