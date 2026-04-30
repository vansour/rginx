use super::super::*;

#[test]
fn local_admin_socket_serves_revision_snapshot() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-admin-uds", |_| return_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = query_admin_socket(&socket_path, AdminRequest::GetRevision)
        .expect("admin socket should return revision");
    assert_eq!(response, AdminResponse::Revision(RevisionSnapshot { revision: 0 }));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn snapshot_command_returns_aggregate_json_snapshot() {
    let listen_addr = reserve_loopback_addr();
    let upstream_addr = spawn_response_server("snapshot upstream ok\n");
    let mut server =
        ServerHarness::spawn("rginx-admin-snapshot", |_| proxy_config(listen_addr, upstream_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let (status, body) =
        fetch_text_response(listen_addr, "/api/demo").expect("proxy request should succeed");
    assert_eq!(status, 200);
    assert_eq!(body, "snapshot upstream ok\n");

    let response = query_admin_socket(
        &socket_path,
        AdminRequest::GetSnapshot { include: None, window_secs: None },
    )
    .expect("admin socket should return aggregate snapshot");
    let AdminResponse::Snapshot(snapshot) = response else {
        panic!("admin socket should return aggregate snapshot");
    };
    assert_eq!(snapshot.schema_version, 15);
    assert!(snapshot.captured_at_unix_ms > 0);
    assert!(snapshot.pid > 0);
    assert_eq!(snapshot.binary_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(snapshot.included_modules, rginx_http::SnapshotModule::all());
    assert_eq!(snapshot.status.as_ref().map(|status| status.listeners.len()), Some(1));
    assert_eq!(snapshot.status.as_ref().map(|status| status.acme.enabled), Some(false));
    assert_eq!(
        snapshot
            .status
            .as_ref()
            .and_then(|status| status.listeners.first())
            .map(|listener| listener.listen_addr),
        Some(listen_addr)
    );
    assert_eq!(snapshot.status.as_ref().map(|status| status.tls.listeners.len()), Some(1));
    assert!(snapshot.counters.as_ref().map(|c| c.downstream_requests).unwrap_or(0) >= 1);
    assert_eq!(snapshot.traffic.as_ref().map(|t| t.listeners.len()), Some(1));
    assert_eq!(snapshot.peer_health.as_ref().map(Vec::len), Some(1));
    assert_eq!(snapshot.upstreams.as_ref().map(Vec::len), Some(1));
    assert_eq!(snapshot.cache.as_ref().map(|cache| cache.zones.len()), Some(0));

    let output = run_rginx(["--config", server.config_path().to_str().unwrap(), "snapshot"]);
    assert!(output.status.success(), "snapshot command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let snapshot: serde_json::Value =
        serde_json::from_str(&stdout).expect("snapshot command should print valid JSON");
    assert_eq!(snapshot["schema_version"], serde_json::Value::from(15));
    assert!(snapshot["captured_at_unix_ms"].as_u64().unwrap_or(0) > 0);
    assert!(snapshot["pid"].as_u64().unwrap_or(0) > 0);
    assert_eq!(snapshot["binary_version"], serde_json::Value::from(env!("CARGO_PKG_VERSION")));
    assert_eq!(snapshot["status"]["listeners"].as_array().map(Vec::len), Some(1));
    assert_eq!(snapshot["status"]["acme"]["enabled"], serde_json::Value::from(false));
    assert_eq!(
        snapshot["status"]["listeners"][0]["listen_addr"],
        serde_json::Value::from(listen_addr.to_string())
    );
    assert_eq!(snapshot["status"]["tls"]["listeners"].as_array().map(Vec::len), Some(1));
    assert!(snapshot["counters"]["downstream_requests"].as_u64().unwrap_or(0) >= 1);
    assert_eq!(snapshot["traffic"]["listeners"].as_array().map(Vec::len), Some(1));
    assert_eq!(snapshot["peer_health"].as_array().map(Vec::len), Some(1));
    assert_eq!(snapshot["upstreams"].as_array().map(Vec::len), Some(1));
    assert_eq!(snapshot["cache"]["zones"].as_array().map(Vec::len), Some(0));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn snapshot_version_command_reports_current_snapshot_version() {
    let listen_addr = reserve_loopback_addr();
    let mut server =
        ServerHarness::spawn("rginx-admin-snapshot-version", |_| return_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = query_admin_socket(&socket_path, AdminRequest::GetSnapshotVersion)
        .expect("admin socket should return snapshot version");
    let AdminResponse::SnapshotVersion(snapshot) = response else {
        panic!("admin socket should return snapshot version");
    };

    let output =
        run_rginx(["--config", server.config_path().to_str().unwrap(), "snapshot-version"]);
    assert!(
        output.status.success(),
        "snapshot-version command should succeed: {}",
        render_output(&output)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let printed_version = stdout
        .lines()
        .find_map(|line| line.strip_prefix("snapshot_version="))
        .and_then(|value| value.parse::<u64>().ok())
        .expect("snapshot-version command should print snapshot version");
    assert!(printed_version >= snapshot.snapshot_version);

    server.shutdown_and_wait(Duration::from_secs(5));
}
