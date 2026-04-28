use super::super::*;

#[test]
fn status_command_reads_local_admin_socket() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-admin-status", |_| return_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));
    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let output = run_rginx(["--config", server.config_path().to_str().unwrap(), "status"]);
    assert!(output.status.success(), "status command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("kind=status"));
    assert!(stdout.contains("revision=0"));
    assert!(stdout.contains("listeners=1"));
    assert!(stdout.contains(&format!("listen_addrs={listen_addr}")));
    assert!(stdout.contains("kind=status_listener"));
    assert!(stdout.contains("listener=default"));
    assert!(stdout.contains(&format!("listen={listen_addr}")));
    assert!(stdout.contains("tls_listeners=1"));
    assert!(stdout.contains("tls_certificates=0"));
    assert!(stdout.contains("tls_expiring_certificates=0"));
    assert!(stdout.contains("cache_zones=0"));
    assert!(stdout.contains("kind=status_cache"));
    assert!(stdout.contains("http3_active_connections=0"));
    assert!(stdout.contains("http3_active_request_streams=0"));
    assert!(stdout.contains("http3_retry_issued_total=0"));
    assert!(stdout.contains("http3_request_body_stream_errors_total=0"));
    assert!(stdout.contains("active_connections=0"));
    assert!(stdout.contains("mtls_listeners=0"));
    assert!(stdout.contains("reload_attempts=0"));
    assert!(stdout.contains("last_reload=-"));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn status_command_reports_explicit_listener_inventory() {
    let http_addr = reserve_loopback_addr();
    let https_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-admin-status-explicit", |_| {
        explicit_listeners_config(&[("http", http_addr), ("https", https_addr)], "ok\n")
    });
    server.wait_for_http_ready(http_addr, Duration::from_secs(5));
    server.wait_for_http_text_response(
        https_addr,
        "example.com",
        "/",
        200,
        "ok\n",
        Duration::from_secs(5),
    );
    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let output = run_rginx(["--config", server.config_path().to_str().unwrap(), "status"]);
    assert!(output.status.success(), "status command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("listeners=2"));
    let status_line =
        stdout.lines().find(|line| line.contains("kind=status")).expect("status line should exist");
    let listen_addrs = status_line
        .split_whitespace()
        .find_map(|field| field.strip_prefix("listen_addrs="))
        .expect("listen_addrs field should exist");
    let mut actual = listen_addrs.split(',').map(str::to_string).collect::<Vec<_>>();
    actual.sort();
    let mut expected = vec![http_addr.to_string(), https_addr.to_string()];
    expected.sort();
    assert_eq!(actual, expected);
    assert!(stdout.contains("kind=status_listener"));
    assert!(stdout.contains("listener=http"));
    assert!(stdout.contains("listener=https"));
    assert!(stdout.contains(&format!("listen={http_addr}")));
    assert!(stdout.contains(&format!("listen={https_addr}")));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn status_command_reports_http3_listener_bindings() {
    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-admin-status-http3",
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

    let output = run_rginx(["--config", server.config_path().to_str().unwrap(), "status"]);
    assert!(output.status.success(), "status command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("listener_bindings=2"));
    assert!(stdout.contains("bind_addrs=tcp://"));
    assert!(stdout.contains("udp://"));
    assert!(stdout.contains("http3=enabled"));
    assert!(stdout.contains("http3_early_data_enabled_listeners=1"));
    assert!(stdout.contains("http3_active_connections=0"));
    assert!(stdout.contains("http3_active_request_streams=0"));
    assert!(stdout.contains("http3_retry_issued_total=0"));
    assert!(stdout.contains("http3_request_accept_errors_total=0"));
    assert!(stdout.contains("http3_request_body_stream_errors_total=0"));
    assert!(stdout.contains("http3_response_stream_errors_total=0"));
    assert!(stdout.contains("http3_early_data_accepted_requests=0"));
    assert!(stdout.contains("http3_early_data_rejected_requests=0"));
    assert!(stdout.contains("kind=status_listener"));
    assert!(stdout.contains("transport_bindings=2"));
    assert!(stdout.contains("kind=status_listener_binding"));
    assert!(stdout.contains("kind=status_listener_binding listener=default listener_id=default"));
    assert!(stdout.contains("binding=tcp"));
    assert!(stdout.contains("binding=udp"));
    assert!(stdout.contains("transport=udp"));
    assert!(stdout.contains("protocols=http3"));
    assert!(stdout.contains("worker_count=2"));
    assert!(stdout.contains("reuse_port_enabled=true"));
    assert!(stdout.contains("advertise_alt_svc=true"));
    assert!(stdout.contains("alt_svc_max_age_secs=7200"));
    assert!(stdout.contains("http3_max_concurrent_streams=128"));
    assert!(stdout.contains("http3_stream_buffer_size=65536"));
    assert!(stdout.contains("http3_active_connection_id_limit=2"));
    assert!(stdout.contains("http3_retry=false"));
    assert!(stdout.contains("http3_host_key_path=-"));
    assert!(stdout.contains("http3_gso=false"));
    assert!(stdout.contains("http3_early_data_enabled=true"));
    assert!(stdout.contains("kind=status_listener_http3"));
    assert!(stdout.contains("retry_issued_total=0"));
    assert!(stdout.contains("request_accept_errors_total=0"));
    assert!(stdout.contains("request_body_stream_errors_total=0"));
    assert!(stdout.contains("kind=status_tls_listener"));
    assert!(stdout.contains("http3_enabled=true"));
    assert!(stdout.contains("http3_versions=TLS1.3"));
    assert!(stdout.contains("http3_alpn_protocols=h3"));
    assert!(stdout.contains("http3_max_concurrent_streams=128"));
    assert!(stdout.contains("http3_stream_buffer_size=65536"));
    assert!(stdout.contains("http3_active_connection_id_limit=2"));
    assert!(stdout.contains("http3_retry=false"));
    assert!(stdout.contains("http3_host_key_path=-"));
    assert!(stdout.contains("http3_gso=false"));
    assert!(stdout.contains("http3_early_data_enabled=true"));
    assert!(stdout.contains("session_resumption_enabled=true"));
    assert!(stdout.contains("session_tickets_enabled=true"));
    assert!(stdout.contains("session_cache_size=256"));
    assert!(stdout.contains("session_ticket_count=2"));

    server.shutdown_and_wait(Duration::from_secs(5));
}
