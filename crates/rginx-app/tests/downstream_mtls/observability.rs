use super::*;

#[test]
fn mtls_access_log_variables_render_client_identity() {
    let fixture = TlsFixture::new("rginx-downstream-mtls-access-log");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-downstream-mtls-access-log",
        &fixture.server_cert_pem,
        &fixture.server_key_pem,
        |temp_dir, cert_path, key_path| {
            let ca_path = temp_dir.join("client-ca.pem");
            std::fs::write(&ca_path, &fixture.ca_cert_pem).expect("CA cert should be written");
            format!(
                "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        access_log_format: Some(\"mtls=$tls_client_authenticated subject=\\\"$tls_client_subject\\\" issuer=\\\"$tls_client_issuer\\\" serial=\\\"$tls_client_serial\\\" chain=$tls_client_chain_length chain_subjects=\\\"$tls_client_chain_subjects\\\" san=\\\"$tls_client_san_dns_names\\\"\"),\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n            client_auth: Some(ServerClientAuthConfig(\n                mode: Optional,\n                ca_cert_path: {:?},\n            )),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"optional mtls\\n\"),\n            ),\n        ),\n    ],\n)\n",
                listen_addr.to_string(),
                cert_path.display().to_string(),
                key_path.display().to_string(),
                ca_path.display().to_string(),
                ready_route = READY_ROUTE_CONFIG,
            )
        },
    );

    wait_for_https_text_response(
        &mut server,
        listen_addr,
        "localhost",
        "/",
        Some((&fixture.client_cert_path, &fixture.client_key_path)),
        200,
        "optional mtls\n",
        Duration::from_secs(5),
    );
    let response = fetch_https_text_response(
        listen_addr,
        "localhost",
        "/",
        Some((&fixture.client_cert_path, &fixture.client_key_path)),
    )
    .expect("authenticated request should succeed");
    assert_eq!(response, (200, "optional mtls\n".to_string()));

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output = server.combined_output();
        if output.contains("mtls=true")
            && output.contains("subject=\"CN=client.example.com\"")
            && output.contains("issuer=\"CN=rginx test ca\"")
            && output.contains("serial=")
            && output.contains("chain=1")
            && output.contains("chain_subjects=\"CN=client.example.com\"")
            && output.contains("san=\"client.example.com\"")
        {
            break;
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for mTLS access log line\n{}", output);
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    server.shutdown_and_wait(Duration::from_secs(5));
}
