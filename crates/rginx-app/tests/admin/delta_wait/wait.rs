use super::super::*;

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
