use super::*;

#[test]
fn status_and_check_report_dynamic_ocsp_refresh_state() {
    let ocsp_requests = Arc::new(AtomicUsize::new(0));
    let ocsp_response_body = Arc::new(Mutex::new(Vec::new()));
    let responder_addr = spawn_ocsp_responder(ocsp_requests.clone(), ocsp_response_body.clone());
    let listen_addr = reserve_loopback_addr();

    let mut server = ServerHarness::spawn("rginx-ocsp-refresh", |temp_dir| {
        let cert_path = temp_dir.join("server-chain.pem");
        let key_path = temp_dir.join("server.key");
        let ocsp_path = temp_dir.join("server.ocsp");

        let ca = generate_ca_cert("rginx-ocsp-test-ca");
        let leaf = generate_leaf_cert_with_ocsp_aia(
            "localhost",
            &ca,
            &format!("http://127.0.0.1:{}/ocsp", responder_addr.port()),
        );
        fs::write(&cert_path, format!("{}{}", leaf.cert.pem(), ca.cert.pem()))
            .expect("certificate chain should be written");
        fs::write(&key_path, leaf.signing_key.serialize_pem())
            .expect("private key should be written");
        fs::write(&ocsp_path, b"").expect("empty OCSP cache file should be written");
        *ocsp_response_body.lock().expect("OCSP response body mutex should lock") =
            build_ocsp_response_for_certificate(&cert_path, &ca);

        format!(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n            ocsp_staple_path: Some({:?}),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n        ),\n    ],\n)\n",
            listen_addr.to_string(),
            cert_path.display().to_string(),
            key_path.display().to_string(),
            ocsp_path.display().to_string(),
            ready_route = READY_ROUTE_CONFIG,
        )
    });
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));
    let config_path = server.config_path().to_path_buf();

    let status_stdout =
        wait_for_command_output(&config_path, &["status"], Duration::from_secs(10), |stdout| {
            stdout.contains("kind=status_tls_ocsp scope=listener:default")
                && stdout.contains("auto_refresh_enabled=true")
                && stdout.contains("cache_loaded=true")
                && stdout.contains("refreshes_total=")
        });
    assert!(status_stdout.contains("responder_urls=http://127.0.0.1:"));
    assert!(status_stdout.contains("staple_path="));
    assert!(status_stdout.contains("last_error=-"));
    assert!(ocsp_requests.load(Ordering::Relaxed) >= 1);

    let check_output = run_rginx_with_config(&config_path, &["check"]);
    assert!(
        check_output.status.success(),
        "check should succeed: {}",
        render_output(&check_output)
    );
    let check_stdout = String::from_utf8_lossy(&check_output.stdout);
    assert!(check_stdout.contains("tls_ocsp scope=listener:default"));
    assert!(check_stdout.contains("auto_refresh_enabled=true"));
    assert!(check_stdout.contains("cache_loaded=true"));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn dynamic_ocsp_refresh_recovers_after_reload() {
    let ocsp_requests = Arc::new(AtomicUsize::new(0));
    let ocsp_response_body = Arc::new(Mutex::new(b"dummy-ocsp-response".to_vec()));
    let valid_response = Arc::new(Mutex::new(Vec::new()));
    let responder_addr = spawn_ocsp_responder(ocsp_requests.clone(), ocsp_response_body.clone());
    let listen_addr = reserve_loopback_addr();

    let mut server = ServerHarness::spawn("rginx-ocsp-refresh-reload-recovery", {
        let valid_response = valid_response.clone();
        move |temp_dir| {
            let cert_path = temp_dir.join("server-chain.pem");
            let key_path = temp_dir.join("server.key");
            let ocsp_path = temp_dir.join("server.ocsp");

            let ca = generate_ca_cert("rginx-ocsp-test-ca");
            let leaf = generate_leaf_cert_with_ocsp_aia(
                "localhost",
                &ca,
                &format!("http://127.0.0.1:{}/ocsp", responder_addr.port()),
            );
            fs::write(&cert_path, format!("{}{}", leaf.cert.pem(), ca.cert.pem()))
                .expect("certificate chain should be written");
            fs::write(&key_path, leaf.signing_key.serialize_pem())
                .expect("private key should be written");
            fs::write(&ocsp_path, b"").expect("empty OCSP cache file should be written");

            *valid_response.lock().expect("valid response mutex should lock") =
                build_ocsp_response_for_certificate(&cert_path, &ca);

            dynamic_ocsp_config(
                listen_addr,
                &cert_path,
                &key_path,
                &ocsp_path,
                "before OCSP reload recovery\n",
            )
        }
    });
    server.wait_for_https_text_response(
        listen_addr,
        &listen_addr.to_string(),
        "/",
        "localhost",
        200,
        "before OCSP reload recovery\n",
        Duration::from_secs(5),
    );
    let config_path = server.config_path().to_path_buf();
    let config_dir = config_path.parent().expect("config path should have parent");
    let cert_path = config_dir.join("server-chain.pem");
    let key_path = config_dir.join("server.key");
    let ocsp_path = config_dir.join("server.ocsp");

    let first_status =
        wait_for_command_output(&config_path, &["status"], Duration::from_secs(10), |stdout| {
            stdout.contains("kind=status_tls_ocsp scope=listener:default")
                && stdout.contains("cache_loaded=false")
                && !stdout.contains("failures_total=0")
                && !stdout.contains("last_error=-")
        });
    assert!(first_status.contains("failed to parse OCSP response"));

    *ocsp_response_body.lock().expect("OCSP response body mutex should lock") =
        valid_response.lock().expect("valid response mutex should lock").clone();
    fs::write(
        &config_path,
        dynamic_ocsp_config(
            listen_addr,
            &cert_path,
            &key_path,
            &ocsp_path,
            "after OCSP reload recovery\n",
        ),
    )
    .expect("reloaded config should be written");
    server.send_signal(libc::SIGHUP);

    server.wait_for_https_text_response(
        listen_addr,
        &listen_addr.to_string(),
        "/",
        "localhost",
        200,
        "after OCSP reload recovery\n",
        Duration::from_secs(5),
    );

    let second_status =
        wait_for_command_output(&config_path, &["status"], Duration::from_secs(10), |stdout| {
            stdout.contains("kind=status_tls_ocsp scope=listener:default")
                && stdout.contains("cache_loaded=true")
                && stdout.contains("refreshes_total=1")
                && !stdout.contains("failures_total=0")
                && stdout.contains("last_error=-")
        });
    assert!(second_status.contains("cache_loaded=true"));
    assert!(ocsp_requests.load(Ordering::Relaxed) >= 3);
    assert_eq!(
        fs::read(&ocsp_path).expect("recovered OCSP cache file should be readable"),
        valid_response.lock().expect("valid response mutex should lock").clone(),
        "reload-triggered OCSP refresh should persist the recovered staple"
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}
