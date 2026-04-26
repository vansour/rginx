use super::super::*;

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
