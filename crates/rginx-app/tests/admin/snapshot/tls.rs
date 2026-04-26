use super::super::*;

#[test]
fn snapshot_includes_certificate_fingerprint_and_chain_details_for_tls_servers() {
    let listen_addr = reserve_loopback_addr();
    let cert = generate_cert("localhost");
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-admin-tls-snapshot",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| {
            format!(
                "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n        ),\n    ],\n)\n",
                listen_addr.to_string(),
                cert_path.display().to_string(),
                key_path.display().to_string(),
                ready_route = READY_ROUTE_CONFIG,
            )
        },
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = query_admin_socket(
        &socket_path,
        AdminRequest::GetSnapshot { include: None, window_secs: None },
    )
    .expect("admin socket should return aggregate snapshot");
    let AdminResponse::Snapshot(snapshot) = response else {
        panic!("admin socket should return aggregate snapshot");
    };
    let certificates =
        snapshot.status.as_ref().map(|status| status.tls.certificates.as_slice()).unwrap_or(&[]);
    let vhost_bindings =
        snapshot.status.as_ref().map(|status| status.tls.vhost_bindings.as_slice()).unwrap_or(&[]);
    let sni_bindings =
        snapshot.status.as_ref().map(|status| status.tls.sni_bindings.as_slice()).unwrap_or(&[]);
    assert_eq!(certificates.len(), 1);
    assert_eq!(vhost_bindings.len(), 1);
    assert_eq!(sni_bindings.len(), 1);
    let certificate = &certificates[0];
    assert_eq!(certificate.scope, "listener:default");
    assert!(certificate.subject.is_some());
    assert_eq!(certificate.san_dns_names, vec!["localhost".to_string()]);
    assert_eq!(certificate.subject, certificate.issuer);
    assert!(certificate.fingerprint_sha256.as_ref().is_some_and(|value| value.len() == 64));
    assert_eq!(certificate.chain_length, 1);
    assert_eq!(certificate.chain_subjects.len(), 1);
    assert_eq!(certificate.chain_subjects[0], certificate.subject.clone().unwrap_or_default());
    assert!(certificate.chain_diagnostics.is_empty());

    let status_output = run_rginx(["--config", server.config_path().to_str().unwrap(), "status"]);
    assert!(
        status_output.status.success(),
        "status command should succeed: {}",
        render_output(&status_output)
    );
    let status_stdout = String::from_utf8_lossy(&status_output.stdout);
    assert!(status_stdout.contains("kind=status_tls_certificate"));
    assert!(status_stdout.contains("kind=status_tls_vhost_binding"));
    assert!(status_stdout.contains("kind=status_tls_sni_binding"));
    assert!(status_stdout.contains("sha256="));
    assert!(status_stdout.contains("server_names=localhost"));
    assert!(status_stdout.contains("not_before_unix_ms="));
    assert!(status_stdout.contains("not_after_unix_ms="));
    assert!(status_stdout.contains("expires_in_days="));
    assert!(status_stdout.contains("chain_subjects="));
    assert!(status_stdout.contains("selected_as_default_for_listeners=default"));
    assert!(status_stdout.contains("ocsp_staple_configured=false"));
    assert!(status_stdout.contains("additional_certificate_count=0"));
    assert!(status_stdout.contains("san_dns_names=localhost"));

    server.shutdown_and_wait(Duration::from_secs(5));
}
