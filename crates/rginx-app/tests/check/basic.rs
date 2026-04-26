use super::*;

#[test]
fn check_succeeds_without_binding_listener() {
    let reserved = TcpListener::bind(("127.0.0.1", 0)).expect("reserved listener should bind");
    let listen_addr = reserved.local_addr().expect("listener addr should be available");

    let temp_dir = temp_dir("rginx-check-test");
    fs::create_dir_all(&temp_dir).expect("temp test dir should be created");
    let config_path = temp_dir.join("valid.ron");
    write_return_config(&config_path, listen_addr, "checked\n");

    let output = run_rginx(["check", "--config", config_path.to_str().unwrap()]);

    assert!(
        output.status.success(),
        "check should succeed without binding the listener: {}",
        render_output(&output)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("configuration is valid"));
    assert!(stdout.contains("listener_model=legacy"));
    assert!(stdout.contains("listeners=1"));
    assert!(stdout.contains(&format!("listen_addrs={listen_addr}")));
    assert!(stdout.contains("check_listener id=default name=default"));
    assert!(stdout.contains(&format!("listen={listen_addr}")));
    assert!(stdout.contains("worker_threads=auto"));
    assert!(stdout.contains("accept_workers=1"));
    assert!(stdout.contains(
        "reload_requires_restart_for=listen,server.http3.listen,listeners[].listen,listeners[].http3.listen,runtime.worker_threads,runtime.accept_workers"
    ));
    assert!(stdout.contains(
        "reload_tls_updates=server.tls,server.http3.advertise_alt_svc,server.http3.alt_svc_max_age_secs,listeners[].tls,listeners[].http3.advertise_alt_svc,listeners[].http3.alt_svc_max_age_secs,servers[].tls,upstreams[].tls,upstreams[].server_name,upstreams[].server_name_override"
    ));
    assert!(stdout.contains("tls_expiring_certificates=-"));

    drop(reserved);
    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn nginx_style_t_flag_succeeds_without_binding_listener() {
    let reserved = TcpListener::bind(("127.0.0.1", 0)).expect("reserved listener should bind");
    let listen_addr = reserved.local_addr().expect("listener addr should be available");

    let temp_dir = temp_dir("rginx-check-test");
    fs::create_dir_all(&temp_dir).expect("temp test dir should be created");
    let config_path = temp_dir.join("valid.ron");
    write_return_config(&config_path, listen_addr, "checked\n");

    let output = run_rginx(["-t", "--config", config_path.to_str().unwrap()]);

    assert!(output.status.success(), "-t should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("configuration is valid"));
    assert!(stdout.contains("listener_model=legacy"));
    assert!(stdout.contains(&format!("listen_addrs={listen_addr}")));
    assert!(stdout.contains(&format!("listen={listen_addr}")));

    drop(reserved);
    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn check_returns_error_for_invalid_config() {
    let temp_dir = temp_dir("rginx-check-test");
    fs::create_dir_all(&temp_dir).expect("temp test dir should be created");
    let config_path = temp_dir.join("invalid.ron");
    fs::write(
        &config_path,
        "Config(runtime: RuntimeConfig(shutdown_timeout_secs: 0), server: ServerConfig(listen: \"127.0.0.1:8080\"), upstreams: [], locations: [])",
    )
    .expect("invalid config should be written");

    let output = run_rginx(["check", "--config", config_path.to_str().unwrap()]);

    assert!(!output.status.success(), "check should fail for invalid config");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("shutdown_timeout_secs"));

    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn check_returns_error_for_invalid_server_tls_material() {
    let temp_dir = temp_dir("rginx-check-test");
    fs::create_dir_all(&temp_dir).expect("temp test dir should be created");
    let config_path = temp_dir.join("invalid-tls.ron");
    let cert_path = temp_dir.join("server.crt");
    let key_path = temp_dir.join("server.key");
    fs::write(&cert_path, "not a certificate").expect("invalid cert should be written");
    fs::write(&key_path, "not a private key").expect("invalid key should be written");
    let listen_addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();

    fs::write(
        &config_path,
        format!(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some({:?}),\n            ),\n        ),\n    ],\n)\n",
            listen_addr.to_string(),
            cert_path.display().to_string(),
            key_path.display().to_string(),
            "checked\n"
        ),
    )
    .expect("invalid TLS config should be written");

    let output = run_rginx(["check", "--config", config_path.to_str().unwrap()]);

    assert!(!output.status.success(), "check should fail for invalid server TLS");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to initialize runtime dependencies"));

    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn check_succeeds_for_repository_default_config() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should resolve");
    let config_path = workspace_root.join("configs/rginx.ron");

    let output = run_rginx(["check", "--config", config_path.to_str().unwrap()]);

    assert!(
        output.status.success(),
        "repository default config should validate: {}",
        render_output(&output)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("listen=0.0.0.0:80"));
    assert!(stdout.contains("routes=4"));
    assert!(stdout.contains("vhosts=2"));
    assert!(stdout.contains("upstreams=0"));
}
