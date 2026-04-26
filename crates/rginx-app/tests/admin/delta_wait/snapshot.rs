use super::super::*;

#[test]
fn snapshot_command_can_filter_modules() {
    let listen_addr = reserve_loopback_addr();
    let upstream_addr = spawn_response_server("filtered snapshot upstream ok\n");
    let mut server = ServerHarness::spawn("rginx-admin-snapshot-filtered", |_| {
        proxy_config(listen_addr, upstream_addr)
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = query_admin_socket(
        &socket_path,
        AdminRequest::GetSnapshot {
            include: Some(vec![
                rginx_http::SnapshotModule::Traffic,
                rginx_http::SnapshotModule::Upstreams,
            ]),
            window_secs: Some(300),
        },
    )
    .expect("admin socket should return filtered snapshot");
    let AdminResponse::Snapshot(snapshot) = response else {
        panic!("admin socket should return filtered snapshot");
    };
    assert_eq!(
        snapshot.included_modules,
        vec![rginx_http::SnapshotModule::Traffic, rginx_http::SnapshotModule::Upstreams,]
    );
    assert!(snapshot.status.is_none());
    assert!(snapshot.counters.is_none());
    assert!(snapshot.peer_health.is_none());
    assert!(snapshot.traffic.is_some());
    assert!(snapshot.upstreams.is_some());
    assert_eq!(
        snapshot
            .traffic
            .as_ref()
            .and_then(|traffic| traffic.listeners.first())
            .and_then(|listener| listener.recent_window.as_ref())
            .map(|recent| recent.window_secs),
        Some(300)
    );
    assert_eq!(
        snapshot
            .upstreams
            .as_ref()
            .and_then(|upstreams| upstreams.first())
            .and_then(|upstream| upstream.recent_window.as_ref())
            .map(|recent| recent.window_secs),
        Some(300)
    );

    let output = run_rginx([
        "--config",
        server.config_path().to_str().unwrap(),
        "snapshot",
        "--include",
        "traffic",
        "--include",
        "upstreams",
        "--window-secs",
        "300",
    ]);
    assert!(output.status.success(), "snapshot command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let snapshot: serde_json::Value =
        serde_json::from_str(&stdout).expect("snapshot command should print valid JSON");
    assert_eq!(snapshot["included_modules"], serde_json::json!(["traffic", "upstreams"]));
    assert!(snapshot.get("status").is_none());
    assert!(snapshot.get("counters").is_none());
    assert!(snapshot.get("peer_health").is_none());
    assert!(snapshot.get("traffic").is_some());
    assert!(snapshot.get("upstreams").is_some());
    assert_eq!(
        snapshot["traffic"]["listeners"][0]["recent_window"]["window_secs"],
        serde_json::Value::from(300)
    );
    assert_eq!(
        snapshot["upstreams"][0]["recent_window"]["window_secs"],
        serde_json::Value::from(300)
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}
