use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn enforces_access_control_and_rate_limits_over_http3() {
    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-policy",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| http3_policy_config(listen_addr, cert_path, key_path),
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let allowed = http3_get(listen_addr, "localhost", "/allow", &cert.cert.pem())
        .await
        .expect("allowed request should succeed");
    assert_eq!(allowed.status, StatusCode::OK);
    assert_eq!(body_text(&allowed), "allowed\n");

    let denied = http3_get(listen_addr, "localhost", "/deny", &cert.cert.pem())
        .await
        .expect("denied request should receive a response");
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    assert_eq!(body_text(&denied), "forbidden\n");

    let first = http3_get(listen_addr, "localhost", "/limited", &cert.cert.pem())
        .await
        .expect("first limited request should succeed");
    assert_eq!(first.status, StatusCode::OK);
    let second = http3_get(listen_addr, "localhost", "/limited", &cert.cert.pem())
        .await
        .expect("second limited request should respond");
    assert_eq!(second.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body_text(&second), "hold your horses! too many requests\n");

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread")]
async fn compresses_large_http3_responses_and_preserves_request_id_and_access_log() {
    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-compression",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| http3_compression_config(listen_addr, cert_path, key_path),
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let response = http3_request(
        listen_addr,
        "localhost",
        "GET",
        "/gzip",
        &[("accept-encoding", "gzip"), ("x-request-id", "http3-log-42")],
        None,
        &cert.cert.pem(),
    )
    .await
    .expect("compressed request should succeed");

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.header("content-encoding"), Some("gzip"));
    assert_eq!(response.header("vary"), Some("Accept-Encoding"));
    assert_eq!(response.header("x-request-id"), Some("http3-log-42"));
    assert_eq!(decode_gzip(&response.body), "http3 gzip body\n".repeat(32).into_bytes());

    server.shutdown_and_wait(Duration::from_secs(5));
    let logs = server.combined_output();
    assert!(
        logs.contains("H3 reqid=http3-log-42 version=HTTP/3.0 status=200"),
        "expected HTTP/3 access log entry, got {logs:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn traffic_command_counts_http3_requests() {
    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-traffic",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| http3_return_config(listen_addr, cert_path, key_path),
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));
    wait_for_admin_socket(
        &rginx_runtime::admin::admin_socket_path_for_config(server.config_path()),
        Duration::from_secs(5),
    );

    let response = http3_get(listen_addr, "localhost", "/v3", &cert.cert.pem())
        .await
        .expect("http3 request should succeed");
    assert_eq!(response.status, StatusCode::OK);

    let output = run_rginx(["--config", server.config_path().to_str().unwrap(), "traffic"]);
    assert!(output.status.success(), "traffic command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("kind=traffic_listener"));
    assert!(stdout.contains("kind=traffic_listener_http3"));
    assert!(stdout.contains("listener=default"));
    assert!(stdout.contains("downstream_requests_total=1"));
    assert!(stdout.contains("downstream_responses_total=1"));
    assert!(stdout.contains("retry_issued_total=0"));
    assert!(stdout.contains("request_body_stream_errors_total=0"));

    server.shutdown_and_wait(Duration::from_secs(5));
}
