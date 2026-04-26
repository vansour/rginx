use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn required_client_auth_over_http3_accepts_authenticated_clients_and_rejects_anonymous_clients()
 {
    let fixture = Http3MtlsFixture::new("rginx-http3-required-mtls");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-required-mtls",
        &fixture.server_cert_pem,
        &fixture.server_key_pem,
        |temp_dir, cert_path, key_path| {
            let ca_path = temp_dir.join("client-ca.pem");
            fs::write(&ca_path, &fixture.ca_cert_pem).expect("CA cert should be written");
            http3_client_auth_config(listen_addr, cert_path, key_path, &ca_path, "Required")
        },
    );
    wait_for_admin_socket(
        &rginx_runtime::admin::admin_socket_path_for_config(server.config_path()),
        Duration::from_secs(5),
    );

    wait_for_http3_text_response(
        listen_addr,
        "localhost",
        "/v3",
        Some((&fixture.client_cert_path, &fixture.client_key_path)),
        200,
        "http3 mtls required\n",
        &fixture.ca_cert_pem,
        Duration::from_secs(5),
    )
    .await;

    let anonymous =
        http3_get_with_client_identity(listen_addr, "localhost", "/v3", None, &fixture.ca_cert_pem)
            .await;
    assert!(anonymous.is_err(), "anonymous HTTP/3 client should be rejected");

    let authenticated = http3_get_with_client_identity(
        listen_addr,
        "localhost",
        "/v3",
        Some((&fixture.client_cert_path, &fixture.client_key_path)),
        &fixture.ca_cert_pem,
    )
    .await
    .expect("authenticated HTTP/3 client should succeed");
    assert_eq!(authenticated.status, StatusCode::OK);
    assert_eq!(body_text(&authenticated), "http3 mtls required\n");

    let status_output = run_rginx(["--config", server.config_path().to_str().unwrap(), "status"]);
    assert!(
        status_output.status.success(),
        "status command should succeed: {}",
        render_output(&status_output)
    );
    let status_stdout = String::from_utf8_lossy(&status_output.stdout);
    assert!(status_stdout.contains("mtls_listeners=1"));
    assert!(status_stdout.contains("mtls_required_listeners=1"));
    assert!(parse_flat_u64(&status_stdout, "mtls_authenticated_connections") >= 1);
    assert!(parse_flat_u64(&status_stdout, "mtls_authenticated_requests") >= 1);
    assert!(parse_flat_u64(&status_stdout, "mtls_handshake_failures_missing_client_cert") >= 1);

    let counters_output =
        run_rginx(["--config", server.config_path().to_str().unwrap(), "counters"]);
    assert!(
        counters_output.status.success(),
        "counters command should succeed: {}",
        render_output(&counters_output)
    );
    let counters_stdout = String::from_utf8_lossy(&counters_output.stdout);
    assert!(
        parse_flat_u64(&counters_stdout, "downstream_mtls_authenticated_connections_total") >= 1
    );
    assert!(parse_flat_u64(&counters_stdout, "downstream_mtls_authenticated_requests_total") >= 1);
    assert!(
        parse_flat_u64(
            &counters_stdout,
            "downstream_tls_handshake_failures_missing_client_cert_total"
        ) >= 1
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread")]
async fn optional_client_auth_over_http3_allows_both_anonymous_and_authenticated_clients() {
    let fixture = Http3MtlsFixture::new("rginx-http3-optional-mtls");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-optional-mtls",
        &fixture.server_cert_pem,
        &fixture.server_key_pem,
        |temp_dir, cert_path, key_path| {
            let ca_path = temp_dir.join("client-ca.pem");
            fs::write(&ca_path, &fixture.ca_cert_pem).expect("CA cert should be written");
            http3_client_auth_config(listen_addr, cert_path, key_path, &ca_path, "Optional")
        },
    );

    wait_for_http3_text_response(
        listen_addr,
        "localhost",
        "/v3",
        None,
        200,
        "http3 mtls optional\n",
        &fixture.ca_cert_pem,
        Duration::from_secs(5),
    )
    .await;

    let anonymous =
        http3_get_with_client_identity(listen_addr, "localhost", "/v3", None, &fixture.ca_cert_pem)
            .await
            .expect("anonymous HTTP/3 client should succeed");
    assert_eq!(anonymous.status, StatusCode::OK);
    assert_eq!(body_text(&anonymous), "http3 mtls optional\n");

    let authenticated = http3_get_with_client_identity(
        listen_addr,
        "localhost",
        "/v3",
        Some((&fixture.client_cert_path, &fixture.client_key_path)),
        &fixture.ca_cert_pem,
    )
    .await
    .expect("authenticated HTTP/3 client should succeed");
    assert_eq!(authenticated.status, StatusCode::OK);
    assert_eq!(body_text(&authenticated), "http3 mtls optional\n");

    server.shutdown_and_wait(Duration::from_secs(5));
}
