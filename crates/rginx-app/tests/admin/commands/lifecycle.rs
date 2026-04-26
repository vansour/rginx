use super::super::*;

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
