use super::*;

#[test]
fn required_client_cert_rejects_anonymous_clients_and_accepts_authenticated_clients() {
    let fixture = TlsFixture::new("rginx-downstream-mtls-required");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-downstream-mtls-required",
        &fixture.server_cert_pem,
        &fixture.server_key_pem,
        |temp_dir, cert_path, key_path| {
            let ca_path = temp_dir.join("client-ca.pem");
            std::fs::write(&ca_path, &fixture.ca_cert_pem).expect("CA cert should be written");
            required_client_auth_config(listen_addr, cert_path, key_path, &ca_path)
        },
    );

    wait_for_https_text_response(
        &mut server,
        listen_addr,
        "localhost",
        "/",
        Some((&fixture.client_cert_path, &fixture.client_key_path)),
        200,
        "required mtls\n",
        Duration::from_secs(5),
    );

    let anonymous = fetch_https_text_response(listen_addr, "localhost", "/", None);
    assert!(anonymous.is_err(), "anonymous TLS client should be rejected");

    let authenticated = fetch_https_text_response(
        listen_addr,
        "localhost",
        "/",
        Some((&fixture.client_cert_path, &fixture.client_key_path)),
    )
    .expect("mTLS-authenticated client should succeed");
    assert_eq!(authenticated, (200, "required mtls\n".to_string()));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn optional_client_cert_allows_both_anonymous_and_authenticated_clients() {
    let fixture = TlsFixture::new("rginx-downstream-mtls-optional");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-downstream-mtls-optional",
        &fixture.server_cert_pem,
        &fixture.server_key_pem,
        |temp_dir, cert_path, key_path| {
            let ca_path = temp_dir.join("client-ca.pem");
            std::fs::write(&ca_path, &fixture.ca_cert_pem).expect("CA cert should be written");
            optional_client_auth_config(listen_addr, cert_path, key_path, &ca_path)
        },
    );

    wait_for_https_text_response(
        &mut server,
        listen_addr,
        "localhost",
        "/",
        None,
        200,
        "optional mtls\n",
        Duration::from_secs(5),
    );

    let anonymous =
        fetch_https_text_response(listen_addr, "localhost", "/", None).expect("anonymous client");
    assert_eq!(anonymous, (200, "optional mtls\n".to_string()));

    let authenticated = fetch_https_text_response(
        listen_addr,
        "localhost",
        "/",
        Some((&fixture.client_cert_path, &fixture.client_key_path)),
    )
    .expect("authenticated client");
    assert_eq!(authenticated, (200, "optional mtls\n".to_string()));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn required_mtls_updates_admin_status_and_counters() {
    let fixture = TlsFixture::new("rginx-downstream-mtls-admin");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-downstream-mtls-admin",
        &fixture.server_cert_pem,
        &fixture.server_key_pem,
        |temp_dir, cert_path, key_path| {
            let ca_path = temp_dir.join("client-ca.pem");
            std::fs::write(&ca_path, &fixture.ca_cert_pem).expect("CA cert should be written");
            required_client_auth_config(listen_addr, cert_path, key_path, &ca_path)
        },
    );

    wait_for_https_text_response(
        &mut server,
        listen_addr,
        "localhost",
        "/",
        Some((&fixture.client_cert_path, &fixture.client_key_path)),
        200,
        "required mtls\n",
        Duration::from_secs(5),
    );

    let anonymous = fetch_https_text_response(listen_addr, "localhost", "/", None);
    assert!(anonymous.is_err(), "anonymous TLS client should be rejected");

    let authenticated = fetch_https_text_response(
        listen_addr,
        "localhost",
        "/",
        Some((&fixture.client_cert_path, &fixture.client_key_path)),
    )
    .expect("authenticated client should succeed");
    assert_eq!(authenticated, (200, "required mtls\n".to_string()));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let status = match query_admin_socket(&socket_path, AdminRequest::GetStatus)
        .expect("admin status should succeed")
    {
        AdminResponse::Status(status) => status,
        other => panic!("unexpected admin response: {other:?}"),
    };
    assert_eq!(status.mtls.configured_listeners, 1);
    assert_eq!(status.mtls.required_listeners, 1);
    assert_eq!(status.mtls.optional_listeners, 0);
    assert!(status.mtls.authenticated_connections >= 1);
    assert!(status.mtls.authenticated_requests >= 1);
    assert!(status.mtls.handshake_failures_missing_client_cert >= 1);

    let counters = match query_admin_socket(&socket_path, AdminRequest::GetCounters)
        .expect("admin counters should succeed")
    {
        AdminResponse::Counters(counters) => counters,
        other => panic!("unexpected admin response: {other:?}"),
    };
    assert!(counters.downstream_mtls_authenticated_connections >= 1);
    assert!(counters.downstream_mtls_authenticated_requests >= 1);
    assert!(counters.downstream_tls_handshake_failures_missing_client_cert >= 1);

    server.shutdown_and_wait(Duration::from_secs(5));
}
