use super::super::*;

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
