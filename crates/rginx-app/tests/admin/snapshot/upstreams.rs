use super::super::*;

#[test]
fn status_and_upstreams_commands_report_upstream_tls_diagnostics() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-admin-upstream-tls", |temp_dir| {
        let ca = generate_ca_cert("admin-upstream-ca");
        let crl = generate_crl(&ca, 42);
        let client_identity = generate_cert("upstream-client");
        let ca_path = temp_dir.join("upstream-ca.pem");
        let crl_path = temp_dir.join("upstream.crl.pem");
        let client_cert_path = temp_dir.join("upstream-client.crt");
        let client_key_path = temp_dir.join("upstream-client.key");
        std::fs::write(&ca_path, ca.cert.pem()).expect("upstream CA should be written");
        std::fs::write(&crl_path, crl.pem().expect("CRL PEM should encode"))
            .expect("upstream CRL should be written");
        std::fs::write(&client_cert_path, client_identity.cert.pem())
            .expect("upstream client cert should be written");
        std::fs::write(&client_key_path, client_identity.signing_key.serialize_pem())
            .expect("upstream client key should be written");

        format!(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: \"https://127.0.0.1:9443\",\n                ),\n            ],\n            tls: Some(UpstreamTlsConfig(\n                verify: CustomCa(\n                    ca_cert_path: {:?},\n                ),\n                versions: Some([Tls12, Tls13]),\n                verify_depth: Some(2),\n                crl_path: Some({:?}),\n                client_cert_path: Some({:?}),\n                client_key_path: Some({:?}),\n            )),\n            protocol: Http2,\n            server_name: Some(false),\n            server_name_override: Some(\"api.internal.example\"),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n        ),\n    ],\n)\n",
            listen_addr.to_string(),
            ca_path.display().to_string(),
            crl_path.display().to_string(),
            client_cert_path.display().to_string(),
            client_key_path.display().to_string(),
            ready_route = READY_ROUTE_CONFIG,
        )
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = query_admin_socket(&socket_path, AdminRequest::GetStatus)
        .expect("admin socket should return status");
    let AdminResponse::Status(status) = response else {
        panic!("admin socket should return status");
    };
    assert_eq!(status.upstream_tls.len(), 1);
    let upstream_tls = &status.upstream_tls[0];
    assert_eq!(upstream_tls.upstream_name, "backend");
    assert_eq!(upstream_tls.protocol, "http2");
    assert_eq!(upstream_tls.verify_mode, "custom_ca");
    assert_eq!(
        upstream_tls.tls_versions.as_deref(),
        Some(&["TLS1.2".to_string(), "TLS1.3".to_string()][..])
    );
    assert!(!upstream_tls.server_name_enabled);
    assert_eq!(upstream_tls.server_name_override.as_deref(), Some("api.internal.example"));
    assert_eq!(upstream_tls.verify_depth, Some(2));
    assert!(upstream_tls.crl_configured);
    assert!(upstream_tls.client_identity_configured);

    let response =
        query_admin_socket(&socket_path, AdminRequest::GetUpstreamStats { window_secs: None })
            .expect("admin socket should return upstream stats");
    let AdminResponse::UpstreamStats(upstreams) = response else {
        panic!("admin socket should return upstream stats");
    };
    assert_eq!(upstreams.len(), 1);
    assert_eq!(upstreams[0].tls, status.upstream_tls[0]);

    let status_output = run_rginx(["--config", server.config_path().to_str().unwrap(), "status"]);
    assert!(
        status_output.status.success(),
        "status command should succeed: {}",
        render_output(&status_output)
    );
    let status_stdout = String::from_utf8_lossy(&status_output.stdout);
    assert!(status_stdout.contains(
        "kind=status_upstream_tls upstream=backend protocol=http2 verify_mode=custom_ca"
    ));
    assert!(status_stdout.contains("tls_versions=TLS1.2,TLS1.3"));
    assert!(status_stdout.contains("server_name_enabled=false"));
    assert!(status_stdout.contains("server_name_override=api.internal.example"));
    assert!(status_stdout.contains("verify_depth=2"));
    assert!(status_stdout.contains("crl_configured=true"));
    assert!(status_stdout.contains("client_identity_configured=true"));

    let upstreams_output =
        run_rginx(["--config", server.config_path().to_str().unwrap(), "upstreams"]);
    assert!(
        upstreams_output.status.success(),
        "upstreams command should succeed: {}",
        render_output(&upstreams_output)
    );
    let upstreams_stdout = String::from_utf8_lossy(&upstreams_output.stdout);
    assert!(upstreams_stdout.contains("kind=upstream_stats upstream=backend"));
    assert!(upstreams_stdout.contains("tls_protocol=http2"));
    assert!(upstreams_stdout.contains("tls_verify_mode=custom_ca"));
    assert!(upstreams_stdout.contains("tls_versions=TLS1.2,TLS1.3"));
    assert!(upstreams_stdout.contains("tls_server_name_enabled=false"));
    assert!(upstreams_stdout.contains("tls_server_name_override=api.internal.example"));
    assert!(upstreams_stdout.contains("tls_verify_depth=2"));
    assert!(upstreams_stdout.contains("tls_crl_configured=true"));
    assert!(upstreams_stdout.contains("tls_client_identity_configured=true"));
    assert!(upstreams_stdout.contains("tls_failures_unknown_ca_total=0"));
    assert!(upstreams_stdout.contains("tls_failures_bad_certificate_total=0"));
    assert!(upstreams_stdout.contains("tls_failures_certificate_revoked_total=0"));
    assert!(upstreams_stdout.contains("tls_failures_verify_depth_exceeded_total=0"));

    server.shutdown_and_wait(Duration::from_secs(5));
}
