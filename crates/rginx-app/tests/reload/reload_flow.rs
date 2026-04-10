use super::*;

#[test]
fn sighup_reload_applies_updated_routes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "before reload\n");

    server.wait_for_body(listen_addr, "before reload\n", Duration::from_secs(5));

    server.write_return_config(listen_addr, "after reload\n");
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(listen_addr, "after reload\n", Duration::from_secs(5));
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn nginx_style_reload_command_applies_updated_routes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "before reload\n");

    server.wait_for_body(listen_addr, "before reload\n", Duration::from_secs(5));

    server.write_return_config(listen_addr, "after reload\n");
    let output = server.send_cli_signal("reload");

    assert!(output.status.success(), "rginx -s reload should succeed: {}", render_output(&output));

    server.wait_for_body(listen_addr, "after reload\n", Duration::from_secs(5));
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn nginx_style_quit_command_stops_the_server() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "before quit\n");

    server.wait_for_body(listen_addr, "before quit\n", Duration::from_secs(5));

    let output = server.send_cli_signal("quit");
    assert!(output.status.success(), "rginx -s quit should succeed: {}", render_output(&output));

    let status = server.wait_for_exit(Duration::from_secs(5));
    assert!(status.success(), "rginx should exit cleanly after quit: {status}");
}

#[test]
fn sighup_reload_adds_explicit_listener_without_restart() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let http_addr = reserve_loopback_addr();
    let admin_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn_with_config(
        "rginx-reload-add-listener",
        explicit_listeners_config(&[("http", http_addr)], "before add\n"),
    );

    server.wait_for_body(http_addr, "before add\n", Duration::from_secs(5));
    assert_unreachable(admin_addr, Duration::from_millis(500));

    server.write_config(explicit_listeners_config(
        &[("http", http_addr), ("admin", admin_addr)],
        "after add\n",
    ));
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(http_addr, "after add\n", Duration::from_secs(5));
    server.wait_for_body(admin_addr, "after add\n", Duration::from_secs(5));
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn sighup_reload_removes_explicit_listener_without_restart() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let http_addr = reserve_loopback_addr();
    let admin_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn_with_config(
        "rginx-reload-remove-listener",
        explicit_listeners_config(&[("http", http_addr), ("admin", admin_addr)], "before remove\n"),
    );

    server.wait_for_body(http_addr, "before remove\n", Duration::from_secs(5));
    server.wait_for_body(admin_addr, "before remove\n", Duration::from_secs(5));

    server.write_config(explicit_listeners_config(&[("http", http_addr)], "after remove\n"));
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(http_addr, "after remove\n", Duration::from_secs(5));
    assert_unreachable(admin_addr, Duration::from_secs(2));
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn removed_listener_drains_in_flight_request_before_going_unreachable() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let http_addr = reserve_loopback_addr();
    let drain_addr = reserve_loopback_addr();
    let (ready_tx, ready_rx) = mpsc::channel();
    let upstream_addr =
        spawn_delayed_response_server(Duration::from_millis(300), "draining\n", Some(ready_tx));
    let mut server = TestServer::spawn_with_config(
        "rginx-reload-drain-listener",
        explicit_listeners_proxy_config(
            &[("http", http_addr), ("drain", drain_addr)],
            upstream_addr,
        ),
    );

    server.wait_for_body(http_addr, "draining\n", Duration::from_secs(5));
    server.wait_for_body(drain_addr, "draining\n", Duration::from_secs(5));
    while ready_rx.try_recv().is_ok() {}

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        tx.send(fetch_text_response_with_timeout(drain_addr, "/", Duration::from_secs(3)))
            .expect("result channel should remain available");
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).expect("in-flight request should reach upstream");

    server.write_config(explicit_listeners_proxy_config(&[("http", http_addr)], upstream_addr));
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(http_addr, "draining\n", Duration::from_secs(5));
    let result = rx.recv_timeout(Duration::from_secs(5)).expect("in-flight request should finish");
    let (status, body) = result.expect("in-flight request should succeed");
    assert_eq!(status, 200);
    assert_eq!(body, "draining\n");

    assert_unreachable(drain_addr, Duration::from_secs(2));
    server.shutdown_and_wait(Duration::from_secs(5));
}
