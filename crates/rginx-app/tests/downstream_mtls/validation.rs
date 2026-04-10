use super::*;

#[test]
fn required_mtls_rejects_client_chain_exceeding_verify_depth() {
    let dir = temp_dir("rginx-downstream-mtls-verify-depth");
    std::fs::create_dir_all(&dir).expect("fixture temp dir should be created");

    let ca = generate_ca();
    let intermediate = generate_intermediate_ca("rginx test intermediate", &ca);
    let server = generate_leaf_cert("localhost", &ca, ExtendedKeyUsagePurpose::ServerAuth);
    let client = generate_leaf_cert_with_serial(
        "client-depth.example.com",
        &intermediate,
        ExtendedKeyUsagePurpose::ClientAuth,
        100,
    );

    let client_cert_path = dir.join("client-chain.crt");
    let client_key_path = dir.join("client.key");
    std::fs::write(&client_cert_path, format!("{}{}", client.cert.pem(), intermediate.cert.pem()))
        .expect("client chain should be written");
    std::fs::write(&client_key_path, client.signing_key.serialize_pem())
        .expect("client key should be written");

    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-downstream-mtls-verify-depth",
        &server.cert.pem(),
        &server.signing_key.serialize_pem(),
        |temp_dir, cert_path, key_path| {
            let ca_path = temp_dir.join("client-ca.pem");
            std::fs::write(&ca_path, ca.cert.pem()).expect("CA cert should be written");
            common_client_auth_config_with_extra(
                listen_addr,
                cert_path,
                key_path,
                &ca_path,
                "Required",
                "verify depth mtls\n",
                "                verify_depth: Some(1),\n",
            )
        },
    );

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let result = fetch_https_text_response(
        listen_addr,
        "localhost",
        "/",
        Some((&client_cert_path, &client_key_path)),
    );
    assert!(result.is_err(), "client chain deeper than verify_depth should be rejected");

    let status = match query_admin_socket(&socket_path, AdminRequest::GetStatus)
        .expect("admin status should succeed")
    {
        AdminResponse::Status(status) => status,
        other => panic!("unexpected admin response: {other:?}"),
    };
    assert_eq!(status.tls.listeners[0].client_auth_verify_depth, Some(1));
    assert!(status.mtls.handshake_failures_verify_depth_exceeded >= 1);

    let counters = match query_admin_socket(&socket_path, AdminRequest::GetCounters)
        .expect("admin counters should succeed")
    {
        AdminResponse::Counters(counters) => counters,
        other => panic!("unexpected admin response: {other:?}"),
    };
    assert!(counters.downstream_tls_handshake_failures_verify_depth_exceeded >= 1);

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn required_mtls_rejects_revoked_client_certificates_via_crl() {
    let dir = temp_dir("rginx-downstream-mtls-crl");
    std::fs::create_dir_all(&dir).expect("fixture temp dir should be created");

    let ca = generate_ca();
    let server_leaf = generate_leaf_cert("localhost", &ca, ExtendedKeyUsagePurpose::ServerAuth);
    let revoked_client = generate_leaf_cert_with_serial(
        "client-revoked.example.com",
        &ca,
        ExtendedKeyUsagePurpose::ClientAuth,
        42,
    );
    let crl = generate_client_auth_crl(&ca, 42);

    let client_cert_path = dir.join("client.crt");
    let client_key_path = dir.join("client.key");
    std::fs::write(&client_cert_path, revoked_client.cert.pem())
        .expect("client cert should be written");
    std::fs::write(&client_key_path, revoked_client.signing_key.serialize_pem())
        .expect("client key should be written");

    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-downstream-mtls-crl",
        &server_leaf.cert.pem(),
        &server_leaf.signing_key.serialize_pem(),
        |temp_dir, cert_path, key_path| {
            let ca_path = temp_dir.join("client-ca.pem");
            let crl_path = temp_dir.join("client-auth.crl.pem");
            std::fs::write(&ca_path, ca.cert.pem()).expect("CA cert should be written");
            std::fs::write(&crl_path, crl.pem().expect("CRL PEM should encode"))
                .expect("CRL should be written");
            common_client_auth_config_with_extra(
                listen_addr,
                cert_path,
                key_path,
                &ca_path,
                "Required",
                "crl mtls\n",
                &format!("                crl_path: Some({:?}),\n", crl_path.display().to_string()),
            )
        },
    );

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let result = fetch_https_text_response(
        listen_addr,
        "localhost",
        "/",
        Some((&client_cert_path, &client_key_path)),
    );
    assert!(result.is_err(), "revoked client certificate should be rejected");

    let status = match query_admin_socket(&socket_path, AdminRequest::GetStatus)
        .expect("admin status should succeed")
    {
        AdminResponse::Status(status) => status,
        other => panic!("unexpected admin response: {other:?}"),
    };
    assert!(status.tls.listeners[0].client_auth_crl_configured);
    assert!(status.mtls.handshake_failures_certificate_revoked >= 1);

    let counters = match query_admin_socket(&socket_path, AdminRequest::GetCounters)
        .expect("admin counters should succeed")
    {
        AdminResponse::Counters(counters) => counters,
        other => panic!("unexpected admin response: {other:?}"),
    };
    assert!(counters.downstream_tls_handshake_failures_certificate_revoked >= 1);

    server.shutdown_and_wait(Duration::from_secs(5));
}
