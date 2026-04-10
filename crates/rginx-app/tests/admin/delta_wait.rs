use super::*;

#[test]
fn wait_command_returns_new_snapshot_version_after_local_activity() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-admin-wait", |_| return_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = query_admin_socket(&socket_path, AdminRequest::GetSnapshotVersion)
        .expect("admin socket should return snapshot version");
    let AdminResponse::SnapshotVersion(snapshot) = response else {
        panic!("admin socket should return snapshot version");
    };
    let since_version = snapshot.snapshot_version;

    let (status, body) =
        fetch_text_response(listen_addr, "/").expect("root request should succeed");
    assert_eq!(status, 200);
    assert_eq!(body, "ok\n");

    let response = query_admin_socket(
        &socket_path,
        AdminRequest::WaitForSnapshotChange { since_version, timeout_ms: Some(500) },
    )
    .expect("admin socket should wait for snapshot change");
    let AdminResponse::SnapshotVersion(snapshot) = response else {
        panic!("admin socket should return snapshot version");
    };
    assert!(snapshot.snapshot_version > since_version);

    let since_version_arg = since_version.to_string();
    let output = run_rginx([
        "--config",
        server.config_path().to_str().unwrap(),
        "wait",
        "--since-version",
        &since_version_arg,
        "--timeout-ms",
        "500",
    ]);
    assert!(output.status.success(), "wait command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let waited_version = stdout
        .lines()
        .find_map(|line| line.strip_prefix("snapshot_version="))
        .and_then(|value| value.parse::<u64>().ok())
        .expect("wait command should print snapshot version");
    assert!(waited_version >= snapshot.snapshot_version);

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn wait_command_returns_same_snapshot_version_after_timeout_without_activity() {
    let listen_addr = reserve_loopback_addr();
    let mut server =
        ServerHarness::spawn("rginx-admin-wait-timeout", |_| return_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = query_admin_socket(&socket_path, AdminRequest::GetSnapshotVersion)
        .expect("admin socket should return snapshot version");
    let AdminResponse::SnapshotVersion(snapshot) = response else {
        panic!("admin socket should return snapshot version");
    };
    let since_version = snapshot.snapshot_version;

    let response = query_admin_socket(
        &socket_path,
        AdminRequest::WaitForSnapshotChange { since_version, timeout_ms: Some(100) },
    )
    .expect("admin socket should return the current snapshot version after timeout");
    let AdminResponse::SnapshotVersion(snapshot) = response else {
        panic!("admin socket should return snapshot version");
    };
    assert_eq!(snapshot.snapshot_version, since_version);

    let since_version_arg = since_version.to_string();
    let output = run_rginx([
        "--config",
        server.config_path().to_str().unwrap(),
        "wait",
        "--since-version",
        &since_version_arg,
        "--timeout-ms",
        "100",
    ]);
    assert!(output.status.success(), "wait command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let waited_version = stdout
        .lines()
        .find_map(|line| line.strip_prefix("snapshot_version="))
        .and_then(|value| value.parse::<u64>().ok())
        .expect("wait command should print snapshot version");
    assert_eq!(waited_version, since_version);

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn delta_command_reports_changed_modules_since_version() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-admin-delta", |_| return_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = query_admin_socket(&socket_path, AdminRequest::GetSnapshotVersion)
        .expect("admin socket should return snapshot version");
    let AdminResponse::SnapshotVersion(snapshot) = response else {
        panic!("admin socket should return snapshot version");
    };
    let since_version = snapshot.snapshot_version;

    let (status, body) =
        fetch_text_response(listen_addr, "/").expect("root request should succeed");
    assert_eq!(status, 200);
    assert_eq!(body, "ok\n");

    let response = query_admin_socket(
        &socket_path,
        AdminRequest::GetDelta { since_version, include: None, window_secs: None },
    )
    .expect("admin socket should return delta");
    let AdminResponse::Delta(delta) = response else {
        panic!("admin socket should return delta");
    };
    assert_eq!(delta.schema_version, 2);
    assert_eq!(delta.since_version, since_version);
    assert!(delta.current_snapshot_version > since_version);
    assert_eq!(delta.included_modules, rginx_http::SnapshotModule::all());
    assert_eq!(delta.status_changed, Some(true));
    assert_eq!(delta.counters_changed, Some(true));
    assert_eq!(delta.traffic_changed, Some(true));
    assert_eq!(delta.traffic_recent_changed, None);
    assert_eq!(delta.peer_health_changed, Some(false));
    assert_eq!(delta.upstreams_changed, Some(false));
    assert_eq!(delta.upstreams_recent_changed, None);
    assert_eq!(delta.changed_listener_ids, Some(vec!["default".to_string()]));
    assert_eq!(delta.changed_vhost_ids, Some(vec!["server".to_string()]));
    let changed_route_ids =
        delta.changed_route_ids.as_ref().expect("delta should report changed routes");
    assert!(
        changed_route_ids.iter().any(|route| route == "server/routes[1]|exact:/"),
        "delta should include the business route change: {changed_route_ids:?}"
    );
    assert!(
        changed_route_ids.iter().all(|route| {
            route == "server/routes[0]|exact:/-/ready" || route == "server/routes[1]|exact:/"
        }),
        "delta should only report root and optional ready route changes: {changed_route_ids:?}"
    );
    assert_eq!(delta.recent_window_secs, None);
    assert_eq!(delta.changed_recent_listener_ids, None);
    assert_eq!(delta.changed_recent_vhost_ids, None);
    assert_eq!(delta.changed_recent_route_ids, None);
    assert_eq!(delta.changed_peer_health_upstream_names, Some(Vec::new()));
    assert_eq!(delta.changed_upstream_names, Some(Vec::new()));
    assert_eq!(delta.changed_recent_upstream_names, None);

    let since_version_arg = since_version.to_string();
    let output = run_rginx([
        "--config",
        server.config_path().to_str().unwrap(),
        "delta",
        "--since-version",
        &since_version_arg,
    ]);
    assert!(output.status.success(), "delta command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let delta: serde_json::Value =
        serde_json::from_str(&stdout).expect("delta command should print valid JSON");
    assert_eq!(delta["schema_version"], serde_json::Value::from(2));
    assert_eq!(delta["since_version"], serde_json::Value::from(since_version));
    assert_eq!(delta["status_changed"], serde_json::Value::from(true));
    assert_eq!(delta["counters_changed"], serde_json::Value::from(true));
    assert_eq!(delta["traffic_changed"], serde_json::Value::from(true));
    assert!(delta.get("traffic_recent_changed").is_none());
    assert_eq!(delta["peer_health_changed"], serde_json::Value::from(false));
    assert_eq!(delta["upstreams_changed"], serde_json::Value::from(false));
    assert!(delta.get("upstreams_recent_changed").is_none());
    assert_eq!(delta["changed_listener_ids"], serde_json::json!(["default"]));
    assert_eq!(delta["changed_vhost_ids"], serde_json::json!(["server"]));
    let changed_route_ids =
        delta["changed_route_ids"].as_array().expect("delta JSON should include changed_route_ids");
    assert!(
        changed_route_ids.iter().any(|route| route == "server/routes[1]|exact:/"),
        "delta JSON should include the business route change: {changed_route_ids:?}"
    );
    assert!(
        changed_route_ids.iter().all(|route| {
            route == "server/routes[0]|exact:/-/ready" || route == "server/routes[1]|exact:/"
        }),
        "delta JSON should only report root and optional ready route changes: {changed_route_ids:?}"
    );
    assert!(delta.get("recent_window_secs").is_none());
    assert!(delta.get("changed_recent_listener_ids").is_none());
    assert!(delta.get("changed_recent_vhost_ids").is_none());
    assert!(delta.get("changed_recent_route_ids").is_none());
    assert_eq!(delta["changed_peer_health_upstream_names"], serde_json::json!([]));
    assert_eq!(delta["changed_upstream_names"], serde_json::json!([]));
    assert!(delta.get("changed_recent_upstream_names").is_none());

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn delta_command_reports_peer_health_changes_for_proxy_activity() {
    let listen_addr = reserve_loopback_addr();
    let upstream_addr = spawn_response_server("delta upstream ok\n");
    let mut server = ServerHarness::spawn("rginx-admin-delta-peer-health", |_| {
        proxy_config(listen_addr, upstream_addr)
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = query_admin_socket(&socket_path, AdminRequest::GetSnapshotVersion)
        .expect("admin socket should return snapshot version");
    let AdminResponse::SnapshotVersion(snapshot) = response else {
        panic!("admin socket should return snapshot version");
    };
    let since_version = snapshot.snapshot_version;

    let (status, body) =
        fetch_text_response(listen_addr, "/api/demo").expect("proxy request should succeed");
    assert_eq!(status, 200);
    assert_eq!(body, "delta upstream ok\n");

    let response = query_admin_socket(
        &socket_path,
        AdminRequest::GetDelta { since_version, include: None, window_secs: None },
    )
    .expect("admin socket should return delta");
    let AdminResponse::Delta(delta) = response else {
        panic!("admin socket should return delta");
    };
    assert_eq!(delta.peer_health_changed, Some(true));
    assert_eq!(delta.upstreams_changed, Some(true));
    assert_eq!(delta.changed_peer_health_upstream_names, Some(vec!["backend".to_string()]));
    assert_eq!(delta.changed_upstream_names, Some(vec!["backend".to_string()]));
    assert_eq!(delta.changed_recent_upstream_names, None);

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn delta_command_can_request_recent_window_summary() {
    let listen_addr = reserve_loopback_addr();
    let upstream_addr = spawn_response_server("delta recent upstream ok\n");
    let mut server = ServerHarness::spawn("rginx-admin-delta-recent", |_| {
        proxy_config(listen_addr, upstream_addr)
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = query_admin_socket(&socket_path, AdminRequest::GetSnapshotVersion)
        .expect("admin socket should return snapshot version");
    let AdminResponse::SnapshotVersion(snapshot) = response else {
        panic!("admin socket should return snapshot version");
    };
    let since_version = snapshot.snapshot_version;

    let (status, body) =
        fetch_text_response(listen_addr, "/api/demo").expect("proxy request should succeed");
    assert_eq!(status, 200);
    assert_eq!(body, "delta recent upstream ok\n");

    let response = query_admin_socket(
        &socket_path,
        AdminRequest::GetDelta {
            since_version,
            include: Some(vec![
                rginx_http::SnapshotModule::Traffic,
                rginx_http::SnapshotModule::Upstreams,
            ]),
            window_secs: Some(300),
        },
    )
    .expect("admin socket should return delta");
    let AdminResponse::Delta(delta) = response else {
        panic!("admin socket should return delta");
    };
    assert_eq!(delta.recent_window_secs, Some(300));
    assert_eq!(delta.traffic_changed, Some(true));
    assert_eq!(delta.traffic_recent_changed, Some(true));
    assert_eq!(delta.upstreams_changed, Some(true));
    assert_eq!(delta.upstreams_recent_changed, Some(true));
    assert_eq!(delta.changed_recent_listener_ids, Some(vec!["default".to_string()]));
    assert_eq!(delta.changed_recent_upstream_names, Some(vec!["backend".to_string()]));

    let since_version_arg = since_version.to_string();
    let output = run_rginx([
        "--config",
        server.config_path().to_str().unwrap(),
        "delta",
        "--since-version",
        &since_version_arg,
        "--include",
        "traffic",
        "--include",
        "upstreams",
        "--window-secs",
        "300",
    ]);
    assert!(output.status.success(), "delta command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let delta: serde_json::Value =
        serde_json::from_str(&stdout).expect("delta command should print valid JSON");
    assert_eq!(delta["recent_window_secs"], serde_json::Value::from(300));
    assert_eq!(delta["traffic_recent_changed"], serde_json::Value::from(true));
    assert_eq!(delta["upstreams_recent_changed"], serde_json::Value::from(true));
    assert_eq!(delta["changed_recent_listener_ids"], serde_json::json!(["default"]));
    assert_eq!(delta["changed_recent_upstream_names"], serde_json::json!(["backend"]));

    server.shutdown_and_wait(Duration::from_secs(5));
}

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
