use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn grpc_timeout_over_http3_upstream_returns_deadline_exceeded() {
    let cert = generate_cert("localhost");
    let shared_dir = TempDirGuard::new("rginx-grpc-http3-timeout-shared");
    let server_cert_path = shared_dir.path().join("server.crt");
    let server_key_path = shared_dir.path().join("server.key");
    fs::write(&server_cert_path, cert.cert.pem()).expect("server cert should be written");
    fs::write(&server_key_path, cert.signing_key.serialize_pem())
        .expect("server key should be written");

    let (upstream_addr, _observed_rx, upstream_task, _upstream_temp_dir) = spawn_h3_grpc_upstream(
        &server_cert_path,
        &server_key_path,
        UpstreamMode::DelayHeaders(Duration::from_secs(2)),
    )
    .await;

    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-grpc-http3-timeout",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| {
            grpc_http3_proxy_config_with_timeout(listen_addr, upstream_addr, cert_path, key_path, 1)
        },
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let response = h3_request(
        listen_addr,
        "localhost",
        "POST",
        GRPC_METHOD_PATH,
        &[(CONTENT_TYPE.as_str(), "application/grpc"), (TE.as_str(), "trailers")],
        Some(Bytes::from_static(GRPC_REQUEST_FRAME)),
        &cert.cert.pem(),
    )
    .await
    .expect("grpc timeout request should succeed");

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.headers.get("grpc-status").map(String::as_str), Some("4"));

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.abort();
    let _ = upstream_task.await;
}
