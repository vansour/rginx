use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn validates_client_address_with_http3_retry_and_creates_host_key() {
    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-retry",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |temp_dir, cert_path, key_path| {
            http3_retry_config(listen_addr, cert_path, key_path, &temp_dir.join("quic/host.key"))
        },
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let response = http3_get(listen_addr, "localhost", "/v3", &cert.cert.pem())
        .await
        .expect("http3 request with retry should succeed");
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(body_text(&response), "http3 return\n");

    let status_output = run_rginx(["--config", server.config_path().to_str().unwrap(), "status"]);
    assert!(
        status_output.status.success(),
        "status command should succeed: {}",
        render_output(&status_output)
    );
    let status_stdout = String::from_utf8_lossy(&status_output.stdout);
    assert!(parse_flat_u64(&status_stdout, "http3_retry_issued_total") >= 1);
    assert!(status_stdout.contains("kind=status_listener_http3"));

    server.shutdown_and_wait(Duration::from_secs(5));

    let host_key_path = server.temp_dir().join("quic/host.key");
    let host_key = fs::read(&host_key_path).expect("http3 host key should be created");
    assert_eq!(host_key.len(), 64);

    let logs = server.combined_output();
    assert!(
        logs.contains("http3 issuing retry to validate client address"),
        "expected retry log entry, got {logs:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn preserves_http3_host_key_across_reload() {
    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-retry-reload",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |temp_dir, cert_path, key_path| {
            http3_retry_config(listen_addr, cert_path, key_path, &temp_dir.join("quic/host.key"))
        },
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let first = http3_get(listen_addr, "localhost", "/v3", &cert.cert.pem())
        .await
        .expect("initial http3 request should succeed");
    assert_eq!(first.status, StatusCode::OK);

    let host_key_path = server.temp_dir().join("quic/host.key");
    let before = fs::read(&host_key_path).expect("host key should exist before reload");
    assert_eq!(before.len(), 64);

    server.send_signal(libc::SIGHUP);
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let second = http3_get(listen_addr, "localhost", "/v3", &cert.cert.pem())
        .await
        .expect("http3 request after reload should succeed");
    assert_eq!(second.status, StatusCode::OK);

    let after = fs::read(&host_key_path).expect("host key should exist after reload");
    assert_eq!(before, after);

    server.shutdown_and_wait(Duration::from_secs(5));
}
