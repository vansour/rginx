use super::*;

#[test]
fn local_admin_socket_serves_revision_snapshot() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-admin-uds", |_| return_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = query_admin_socket(&socket_path, AdminRequest::GetRevision)
        .expect("admin socket should return revision");
    assert_eq!(response, AdminResponse::Revision(RevisionSnapshot { revision: 0 }));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn snapshot_command_returns_aggregate_json_snapshot() {
    let listen_addr = reserve_loopback_addr();
    let upstream_addr = spawn_response_server("snapshot upstream ok\n");
    let mut server =
        ServerHarness::spawn("rginx-admin-snapshot", |_| proxy_config(listen_addr, upstream_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let (status, body) =
        fetch_text_response(listen_addr, "/api/demo").expect("proxy request should succeed");
    assert_eq!(status, 200);
    assert_eq!(body, "snapshot upstream ok\n");

    let response = query_admin_socket(
        &socket_path,
        AdminRequest::GetSnapshot { include: None, window_secs: None },
    )
    .expect("admin socket should return aggregate snapshot");
    let AdminResponse::Snapshot(snapshot) = response else {
        panic!("admin socket should return aggregate snapshot");
    };
    assert_eq!(snapshot.schema_version, 13);
    assert!(snapshot.captured_at_unix_ms > 0);
    assert!(snapshot.pid > 0);
    assert_eq!(snapshot.binary_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(snapshot.included_modules, rginx_http::SnapshotModule::all());
    assert_eq!(snapshot.status.as_ref().map(|status| status.listeners.len()), Some(1));
    assert_eq!(
        snapshot
            .status
            .as_ref()
            .and_then(|status| status.listeners.first())
            .map(|listener| listener.listen_addr),
        Some(listen_addr)
    );
    assert_eq!(snapshot.status.as_ref().map(|status| status.tls.listeners.len()), Some(1));
    assert!(snapshot.counters.as_ref().map(|c| c.downstream_requests).unwrap_or(0) >= 1);
    assert_eq!(snapshot.traffic.as_ref().map(|t| t.listeners.len()), Some(1));
    assert_eq!(snapshot.peer_health.as_ref().map(Vec::len), Some(1));
    assert_eq!(snapshot.upstreams.as_ref().map(Vec::len), Some(1));

    let output = run_rginx(["--config", server.config_path().to_str().unwrap(), "snapshot"]);
    assert!(output.status.success(), "snapshot command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let snapshot: serde_json::Value =
        serde_json::from_str(&stdout).expect("snapshot command should print valid JSON");
    assert_eq!(snapshot["schema_version"], serde_json::Value::from(13));
    assert!(snapshot["captured_at_unix_ms"].as_u64().unwrap_or(0) > 0);
    assert!(snapshot["pid"].as_u64().unwrap_or(0) > 0);
    assert_eq!(snapshot["binary_version"], serde_json::Value::from(env!("CARGO_PKG_VERSION")));
    assert_eq!(snapshot["status"]["listeners"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        snapshot["status"]["listeners"][0]["listen_addr"],
        serde_json::Value::from(listen_addr.to_string())
    );
    assert_eq!(snapshot["status"]["tls"]["listeners"].as_array().map(Vec::len), Some(1));
    assert!(snapshot["counters"]["downstream_requests"].as_u64().unwrap_or(0) >= 1);
    assert_eq!(snapshot["traffic"]["listeners"].as_array().map(Vec::len), Some(1));
    assert_eq!(snapshot["peer_health"].as_array().map(Vec::len), Some(1));
    assert_eq!(snapshot["upstreams"].as_array().map(Vec::len), Some(1));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn snapshot_reports_http3_listener_bindings() {
    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-admin-http3-snapshot",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| {
            format!(
                "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n        accept_workers: Some(2),\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n            early_data: Some(true),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n            allow_early_data: Some(true),\n        ),\n    ],\n)\n",
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

    let listener = snapshot
        .status
        .as_ref()
        .and_then(|status| status.listeners.first())
        .expect("snapshot should include one listener");
    assert_eq!(listener.binding_count, 2);
    assert!(listener.http3_enabled);
    assert_eq!(listener.bindings.len(), 2);
    let udp_binding = listener
        .bindings
        .iter()
        .find(|binding| binding.transport == "udp")
        .expect("snapshot should include udp binding");
    assert_eq!(udp_binding.protocols, vec!["http3".to_string()]);
    assert_eq!(udp_binding.worker_count, 2);
    assert_eq!(udp_binding.reuse_port_enabled, Some(true));
    assert_eq!(udp_binding.advertise_alt_svc, Some(true));
    assert_eq!(udp_binding.alt_svc_max_age_secs, Some(7200));
    assert_eq!(udp_binding.http3_max_concurrent_streams, Some(128));
    assert_eq!(udp_binding.http3_stream_buffer_size, Some(64 * 1024));
    assert_eq!(udp_binding.http3_active_connection_id_limit, Some(2));
    assert_eq!(udp_binding.http3_retry, Some(false));
    assert_eq!(udp_binding.http3_host_key_path, None);
    assert_eq!(udp_binding.http3_gso, Some(false));
    assert_eq!(udp_binding.http3_early_data_enabled, Some(true));
    let http3_runtime =
        listener.http3_runtime.as_ref().expect("snapshot should include http3 runtime");
    assert_eq!(http3_runtime.active_connections, 0);
    assert_eq!(http3_runtime.active_request_streams, 0);
    assert_eq!(http3_runtime.retry_issued_total, 0);
    assert_eq!(http3_runtime.request_accept_errors_total, 0);
    assert_eq!(http3_runtime.request_body_stream_errors_total, 0);
    assert_eq!(http3_runtime.response_stream_errors_total, 0);
    let tls_listener = snapshot
        .status
        .as_ref()
        .and_then(|status| status.tls.listeners.first())
        .expect("snapshot should include one tls listener");
    assert!(tls_listener.http3_enabled);
    assert_eq!(tls_listener.http3_listen_addr, Some(listen_addr));
    assert_eq!(tls_listener.http3_versions, vec!["TLS1.3".to_string()]);
    assert_eq!(tls_listener.http3_alpn_protocols, vec!["h3".to_string()]);
    assert_eq!(tls_listener.http3_max_concurrent_streams, Some(128));
    assert_eq!(tls_listener.http3_stream_buffer_size, Some(64 * 1024));
    assert_eq!(tls_listener.http3_active_connection_id_limit, Some(2));
    assert_eq!(tls_listener.http3_retry, Some(false));
    assert_eq!(tls_listener.http3_host_key_path, None);
    assert_eq!(tls_listener.http3_gso, Some(false));
    assert_eq!(tls_listener.http3_early_data_enabled, Some(true));
    assert_eq!(
        snapshot.status.as_ref().map(|status| status.http3_early_data_enabled_listeners),
        Some(1)
    );
    assert_eq!(
        snapshot.status.as_ref().map(|status| status.http3_early_data_accepted_requests),
        Some(0)
    );
    assert_eq!(
        snapshot.status.as_ref().map(|status| status.http3_early_data_rejected_requests),
        Some(0)
    );

    let output = run_rginx(["--config", server.config_path().to_str().unwrap(), "snapshot"]);
    assert!(output.status.success(), "snapshot command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let snapshot: serde_json::Value =
        serde_json::from_str(&stdout).expect("snapshot command should print valid JSON");
    assert_eq!(snapshot["status"]["listeners"][0]["binding_count"], serde_json::Value::from(2));
    assert_eq!(snapshot["status"]["listeners"][0]["http3_enabled"], serde_json::Value::from(true));
    let udp_json = snapshot["status"]["listeners"][0]["bindings"]
        .as_array()
        .and_then(|bindings| {
            bindings.iter().find(|binding| binding["transport"].as_str() == Some("udp"))
        })
        .expect("snapshot JSON should include udp binding");
    assert_eq!(udp_json["protocols"][0], serde_json::Value::from("http3"));
    assert_eq!(udp_json["worker_count"], serde_json::Value::from(2));
    assert_eq!(udp_json["reuse_port_enabled"], serde_json::Value::from(true));
    assert_eq!(udp_json["http3_max_concurrent_streams"], serde_json::Value::from(128));
    assert_eq!(udp_json["http3_stream_buffer_size"], serde_json::Value::from(64 * 1024));
    assert_eq!(udp_json["http3_active_connection_id_limit"], serde_json::Value::from(2));
    assert_eq!(udp_json["http3_retry"], serde_json::Value::from(false));
    assert_eq!(udp_json["http3_gso"], serde_json::Value::from(false));
    assert_eq!(udp_json["http3_early_data_enabled"], serde_json::Value::from(true));
    assert_eq!(
        snapshot["status"]["listeners"][0]["http3_runtime"]["active_connections"],
        serde_json::Value::from(0)
    );
    assert_eq!(
        snapshot["status"]["listeners"][0]["http3_runtime"]["retry_issued_total"],
        serde_json::Value::from(0)
    );
    assert_eq!(
        snapshot["status"]["http3_early_data_enabled_listeners"],
        serde_json::Value::from(1)
    );
    assert_eq!(
        snapshot["status"]["http3_early_data_accepted_requests"],
        serde_json::Value::from(0)
    );
    assert_eq!(
        snapshot["status"]["http3_early_data_rejected_requests"],
        serde_json::Value::from(0)
    );
    assert_eq!(
        snapshot["status"]["tls"]["listeners"][0]["http3_enabled"],
        serde_json::Value::from(true)
    );
    assert_eq!(
        snapshot["status"]["tls"]["listeners"][0]["http3_listen_addr"],
        serde_json::Value::from(listen_addr.to_string())
    );
    assert_eq!(
        snapshot["status"]["tls"]["listeners"][0]["http3_versions"][0],
        serde_json::Value::from("TLS1.3")
    );
    assert_eq!(
        snapshot["status"]["tls"]["listeners"][0]["http3_alpn_protocols"][0],
        serde_json::Value::from("h3")
    );
    assert_eq!(
        snapshot["status"]["tls"]["listeners"][0]["http3_max_concurrent_streams"],
        serde_json::Value::from(128)
    );
    assert_eq!(
        snapshot["status"]["tls"]["listeners"][0]["http3_stream_buffer_size"],
        serde_json::Value::from(64 * 1024)
    );
    assert_eq!(
        snapshot["status"]["tls"]["listeners"][0]["http3_active_connection_id_limit"],
        serde_json::Value::from(2)
    );
    assert_eq!(
        snapshot["status"]["tls"]["listeners"][0]["http3_retry"],
        serde_json::Value::from(false)
    );
    assert_eq!(
        snapshot["status"]["tls"]["listeners"][0]["http3_gso"],
        serde_json::Value::from(false)
    );
    assert_eq!(
        snapshot["status"]["tls"]["listeners"][0]["http3_early_data_enabled"],
        serde_json::Value::from(true)
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

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

#[test]
fn snapshot_version_command_reports_current_snapshot_version() {
    let listen_addr = reserve_loopback_addr();
    let mut server =
        ServerHarness::spawn("rginx-admin-snapshot-version", |_| return_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = query_admin_socket(&socket_path, AdminRequest::GetSnapshotVersion)
        .expect("admin socket should return snapshot version");
    let AdminResponse::SnapshotVersion(snapshot) = response else {
        panic!("admin socket should return snapshot version");
    };

    let output =
        run_rginx(["--config", server.config_path().to_str().unwrap(), "snapshot-version"]);
    assert!(
        output.status.success(),
        "snapshot-version command should succeed: {}",
        render_output(&output)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(&format!("snapshot_version={}", snapshot.snapshot_version)));

    server.shutdown_and_wait(Duration::from_secs(5));
}
