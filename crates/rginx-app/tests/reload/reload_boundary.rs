use super::*;

#[test]
fn sighup_rejects_listen_address_changes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let initial_addr = reserve_loopback_addr();
    let rejected_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(initial_addr, "stable config\n");

    server.wait_for_body(initial_addr, "stable config\n", Duration::from_secs(5));

    server.write_return_config(rejected_addr, "should not apply\n");
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(initial_addr, "stable config\n", Duration::from_secs(5));
    assert_unreachable(rejected_addr, Duration::from_millis(500));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn sighup_rejects_accept_worker_changes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "stable workers\n");

    server.wait_for_body(listen_addr, "stable workers\n", Duration::from_secs(5));

    server.write_config(return_config_with_runtime(
        listen_addr,
        "should not apply\n",
        "        accept_workers: Some(2),\n",
    ));
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(listen_addr, "stable workers\n", Duration::from_secs(5));
    server.kill_and_wait(Duration::from_secs(5));
}

#[test]
fn sighup_rejects_runtime_worker_thread_changes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "stable runtime\n");

    server.wait_for_body(listen_addr, "stable runtime\n", Duration::from_secs(5));

    server.write_config(return_config_with_runtime(
        listen_addr,
        "should not apply\n",
        "        worker_threads: Some(2),\n",
    ));
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(listen_addr, "stable runtime\n", Duration::from_secs(5));
    server.kill_and_wait(Duration::from_secs(5));
}

#[test]
fn sighup_status_reports_restart_required_fields_for_startup_boundary_changes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "stable runtime\n");

    server.wait_for_body(listen_addr, "stable runtime\n", Duration::from_secs(5));

    server.write_config(return_config_with_runtime(
        listen_addr,
        "should not apply\n",
        "        worker_threads: Some(2),\n        accept_workers: Some(2),\n",
    ));
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(listen_addr, "stable runtime\n", Duration::from_secs(5));
    let status_output = server.run_cli_command(["status"]);
    assert!(
        status_output.status.success(),
        "rginx status should succeed after rejected reload: {}",
        render_output(&status_output)
    );
    let stdout = String::from_utf8_lossy(&status_output.stdout);
    assert!(stdout.contains("reload_failures=1"), "stdout should report reload failure: {stdout}");
    assert!(
        stdout.contains("last_reload_active_revision=0"),
        "stdout should report the preserved active revision: {stdout}"
    );
    assert!(
        stdout.contains("last_reload_rollback_revision=0"),
        "stdout should report rollback preservation: {stdout}"
    );
    assert!(
        stdout.contains("reload requires restart because these startup-boundary fields changed"),
        "stdout should explain restart boundary: {stdout}"
    );
    assert!(
        stdout.contains("runtime.worker_threads"),
        "stdout should mention worker_threads: {stdout}"
    );
    assert!(
        stdout.contains("runtime.accept_workers"),
        "stdout should mention accept_workers: {stdout}"
    );

    server.kill_and_wait(Duration::from_secs(5));
}

#[test]
fn sighup_reload_picks_up_updated_included_fragments() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn_with_setup("rginx-reload-include-test", |temp_dir| {
        fs::write(temp_dir.join("routes.ron"), return_route_fragment("before include reload\n"))
            .expect("initial routes fragment should be written");
        format!(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        // @include \"routes.ron\"\n    ],\n)\n",
            listen_addr.to_string(),
            ready_route = READY_ROUTE_CONFIG,
        )
    });
    let routes_path = server.temp_dir().join("routes.ron");

    server.wait_for_body(listen_addr, "before include reload\n", Duration::from_secs(5));

    fs::write(&routes_path, return_route_fragment("after include reload\n"))
        .expect("updated routes fragment should be written");
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(listen_addr, "after include reload\n", Duration::from_secs(5));
    server.shutdown_and_wait(Duration::from_secs(5));
}
