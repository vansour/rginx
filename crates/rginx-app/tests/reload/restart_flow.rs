use super::*;

#[test]
fn nginx_style_restart_command_applies_listen_address_changes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let initial_addr = reserve_loopback_addr();
    let restarted_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(initial_addr, "before restart\n");

    server.wait_for_body(initial_addr, "before restart\n", Duration::from_secs(5));
    let old_pid = read_pid_file(&server.pid_path());

    server.write_return_config(restarted_addr, "after restart\n");
    let output = server.send_cli_signal("restart");
    assert!(output.status.success(), "rginx -s restart should succeed: {}", render_output(&output));

    let new_pid = wait_for_pid_change(&server.pid_path(), old_pid, Duration::from_secs(10));
    let status = server.wait_for_exit(Duration::from_secs(10));
    assert!(status.success(), "old process should exit cleanly after restart: {status}");

    wait_for_body(restarted_addr, "after restart\n", Duration::from_secs(10));
    assert_unreachable(initial_addr, Duration::from_millis(500));

    let quit = server.send_cli_signal("quit");
    assert!(
        quit.status.success(),
        "rginx -s quit should stop replacement process: {}",
        render_output(&quit)
    );
    wait_for_process_exit(new_pid, Duration::from_secs(10));
}

#[test]
fn nginx_style_restart_command_applies_runtime_worker_changes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "runtime restart\n");

    server.wait_for_body(listen_addr, "runtime restart\n", Duration::from_secs(5));
    let old_pid = read_pid_file(&server.pid_path());

    server.write_config(return_config_with_runtime(
        listen_addr,
        "runtime restart\n",
        "        worker_threads: Some(2),\n        accept_workers: Some(2),\n",
    ));
    let output = server.send_cli_signal("restart");
    assert!(output.status.success(), "rginx -s restart should succeed: {}", render_output(&output));

    let new_pid = wait_for_pid_change(&server.pid_path(), old_pid, Duration::from_secs(10));
    let status = server.wait_for_exit(Duration::from_secs(10));
    assert!(status.success(), "old process should exit cleanly after restart: {status}");

    wait_for_body(listen_addr, "runtime restart\n", Duration::from_secs(10));
    let status_output = server.run_cli_command(["status"]);
    assert!(
        status_output.status.success(),
        "rginx status should succeed after restart: {}",
        render_output(&status_output)
    );
    let stdout = String::from_utf8_lossy(&status_output.stdout);
    assert!(stdout.contains("worker_threads=2"));
    assert!(stdout.contains("accept_workers=2"));

    let quit = server.send_cli_signal("quit");
    assert!(
        quit.status.success(),
        "rginx -s quit should stop replacement process: {}",
        render_output(&quit)
    );
    wait_for_process_exit(new_pid, Duration::from_secs(10));
}

#[test]
fn nginx_style_restart_command_keeps_old_process_running_when_replacement_fails() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "stable runtime\n");

    server.wait_for_body(listen_addr, "stable runtime\n", Duration::from_secs(5));
    let old_pid = read_pid_file(&server.pid_path());

    server.write_config(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 0,\n    ),\n    server: ServerConfig(\n        listen: \"127.0.0.1:0\",\n    ),\n    upstreams: [],\n    locations: [],\n)\n".to_string(),
    );
    let output = server.send_cli_signal("restart");
    assert!(
        output.status.success(),
        "restart signal delivery should still succeed: {}",
        render_output(&output)
    );

    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(read_pid_file(&server.pid_path()), old_pid);
    server.wait_for_body(listen_addr, "stable runtime\n", Duration::from_secs(5));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn sighup_status_reports_tls_certificate_changes_after_rotation() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let initial_cert = generate_cert("localhost");
    let rotated_cert = generate_cert("localhost");
    let mut server = ServerHarness::spawn("rginx-reload-tls-rotation", |temp_dir| {
        let cert_path = temp_dir.join("server.crt");
        let key_path = temp_dir.join("server.key");
        fs::write(&cert_path, initial_cert.cert.pem()).expect("initial cert should be written");
        fs::write(&key_path, initial_cert.signing_key.serialize_pem())
            .expect("initial key should be written");
        tls_return_config(listen_addr, &cert_path, &key_path)
    });

    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let rotated_cert_path = server.temp_dir().join("server-rotated.crt");
    let rotated_key_path = server.temp_dir().join("server-rotated.key");
    fs::write(&rotated_cert_path, rotated_cert.cert.pem()).expect("rotated cert should be written");
    fs::write(&rotated_key_path, rotated_cert.signing_key.serialize_pem())
        .expect("rotated key should be written");
    fs::write(
        server.config_path(),
        tls_return_config(listen_addr, &rotated_cert_path, &rotated_key_path),
    )
    .expect("rotated TLS config should be written");
    server.send_signal(libc::SIGHUP);

    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));
    let status_output = run_cli_command(server.config_path(), ["status"]);
    assert!(
        status_output.status.success(),
        "rginx status should succeed after certificate rotation reload: {}",
        render_output(&status_output)
    );
    let stdout = String::from_utf8_lossy(&status_output.stdout);
    assert!(stdout.contains("reload_successes=1"), "stdout should report reload success: {stdout}");
    assert!(
        stdout.contains("last_reload_tls_certificate_changes=")
            && stdout.contains("listener:default:")
            && stdout.contains("->"),
        "stdout should report TLS certificate changes: {stdout}"
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}
