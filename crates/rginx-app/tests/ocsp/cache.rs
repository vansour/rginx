use super::*;

#[test]
fn invalid_dynamic_ocsp_response_is_rejected_before_cache_write() {
    let ocsp_requests = Arc::new(AtomicUsize::new(0));
    let ocsp_response_body = Arc::new(Mutex::new(b"dummy-ocsp-response".to_vec()));
    let responder_addr = spawn_ocsp_responder(ocsp_requests.clone(), ocsp_response_body);
    let listen_addr = reserve_loopback_addr();

    let mut server = ServerHarness::spawn("rginx-ocsp-invalid-refresh", |temp_dir| {
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
                && stdout.contains("cache_loaded=false")
                && stdout.contains("failures_total=")
                && !stdout.contains("last_error=-")
        });
    assert!(status_stdout.contains("failed to parse OCSP response"));
    assert!(ocsp_requests.load(Ordering::Relaxed) >= 1);

    let cache =
        fs::read(config_path.parent().expect("config path should have parent").join("server.ocsp"))
            .expect("OCSP cache file should be readable");
    assert!(cache.is_empty(), "invalid OCSP response should not be cached");

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn expired_ocsp_cache_is_cleared_when_refresh_fails() {
    let ocsp_requests = Arc::new(AtomicUsize::new(0));
    let ocsp_response_body = Arc::new(Mutex::new(b"dummy-ocsp-response".to_vec()));
    let responder_addr = spawn_ocsp_responder(ocsp_requests.clone(), ocsp_response_body);
    let listen_addr = reserve_loopback_addr();

    let mut server = ServerHarness::spawn("rginx-ocsp-expired-cache", |temp_dir| {
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
        fs::write(
            &ocsp_path,
            build_ocsp_response_for_certificate_with_offsets(
                &cert_path,
                &ca,
                TimeOffset::Before(Duration::from_secs(2 * 24 * 60 * 60)),
                TimeOffset::Before(Duration::from_secs(24 * 60 * 60)),
            ),
        )
        .expect("expired OCSP cache file should be written");

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
    let ocsp_path =
        config_path.parent().expect("config path should have parent").join("server.ocsp");

    let status_stdout =
        wait_for_command_output(&config_path, &["status"], Duration::from_secs(10), |stdout| {
            stdout.contains("kind=status_tls_ocsp scope=listener:default")
                && stdout.contains("cache_loaded=false")
                && stdout.contains("cache_size_bytes=0")
        });
    assert!(
        status_stdout.contains("failed to parse OCSP response")
            || status_stdout.contains("is expired")
    );
    let cache = fs::read(&ocsp_path).expect("OCSP cache file should be readable");
    assert!(cache.is_empty(), "expired OCSP cache should be cleared after refresh failure");
    assert!(ocsp_requests.load(Ordering::Relaxed) >= 1);

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn valid_ocsp_cache_is_retained_when_refresh_fails() {
    let ocsp_requests = Arc::new(AtomicUsize::new(0));
    let ocsp_response_body = Arc::new(Mutex::new(b"dummy-ocsp-response".to_vec()));
    let expected_cache = Arc::new(Mutex::new(Vec::new()));
    let responder_addr = spawn_ocsp_responder(ocsp_requests.clone(), ocsp_response_body);
    let listen_addr = reserve_loopback_addr();

    let mut server = ServerHarness::spawn("rginx-ocsp-valid-cache-retained", {
        let expected_cache = expected_cache.clone();
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

            let valid_response = build_ocsp_response_for_certificate(&cert_path, &ca);
            fs::write(&ocsp_path, &valid_response)
                .expect("valid OCSP cache file should be written");
            *expected_cache.lock().expect("expected cache mutex should lock") = valid_response;

            dynamic_ocsp_config(
                listen_addr,
                &cert_path,
                &key_path,
                &ocsp_path,
                "cache survives refresh failure\n",
            )
        }
    });
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));
    let config_path = server.config_path().to_path_buf();
    let ocsp_path =
        config_path.parent().expect("config path should have parent").join("server.ocsp");

    let status_stdout =
        wait_for_command_output(&config_path, &["status"], Duration::from_secs(10), |stdout| {
            stdout.contains("kind=status_tls_ocsp scope=listener:default")
                && stdout.contains("cache_loaded=true")
                && !stdout.contains("failures_total=0")
                && !stdout.contains("last_error=-")
        });
    assert!(status_stdout.contains("failed to parse OCSP response"));
    assert!(ocsp_requests.load(Ordering::Relaxed) >= 1);
    assert_eq!(
        fs::read(&ocsp_path).expect("OCSP cache file should remain readable"),
        expected_cache.lock().expect("expected cache mutex should lock").clone(),
        "valid cached OCSP staple should be retained after refresh failure"
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}
