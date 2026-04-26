use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn proxies_basic_grpc_over_http3_to_http3_upstreams_with_response_trailers() {
    let cert = generate_cert("localhost");
    let shared_dir = TempDirGuard::new("rginx-grpc-http3-shared");
    let server_cert_path = shared_dir.path().join("server.crt");
    let server_key_path = shared_dir.path().join("server.key");
    fs::write(&server_cert_path, cert.cert.pem()).expect("server cert should be written");
    fs::write(&server_key_path, cert.signing_key.serialize_pem())
        .expect("server key should be written");

    let (upstream_addr, observed_rx, upstream_task, _upstream_temp_dir) =
        spawn_h3_grpc_upstream(&server_cert_path, &server_key_path, UpstreamMode::Immediate).await;

    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-grpc-http3-upstream",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| {
            grpc_http3_proxy_config(listen_addr, upstream_addr, cert_path, key_path)
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
    .expect("grpc over http3 request should succeed");

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        response.headers.get(CONTENT_TYPE.as_str()).map(String::as_str),
        Some("application/grpc")
    );
    assert_eq!(
        response.body,
        Bytes::from_static(GRPC_RESPONSE_FRAME),
        "response headers={:?} trailers={:?}\nlogs:\n{}",
        response.headers,
        response.trailers,
        server.combined_output()
    );
    assert_eq!(
        response
            .trailers
            .as_ref()
            .and_then(|trailers| trailers.get("grpc-status"))
            .and_then(|value| value.to_str().ok()),
        Some("0")
    );
    assert_eq!(
        response
            .trailers
            .as_ref()
            .and_then(|trailers| trailers.get("grpc-message"))
            .and_then(|value| value.to_str().ok()),
        Some("ok")
    );

    let observed = tokio::time::timeout(Duration::from_secs(5), observed_rx)
        .await
        .expect("upstream request should be observed before timeout")
        .expect("upstream observation channel should complete");
    assert_eq!(observed.path, GRPC_METHOD_PATH);
    assert_eq!(observed.content_type.as_deref(), Some("application/grpc"));

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.await.expect("upstream h3 grpc task should finish");
}

#[tokio::test(flavor = "multi_thread")]
async fn proxies_grpc_web_binary_over_http3_to_http3_upstreams() {
    let cert = generate_cert("localhost");
    let shared_dir = TempDirGuard::new("rginx-grpc-web-http3-shared");
    let server_cert_path = shared_dir.path().join("server.crt");
    let server_key_path = shared_dir.path().join("server.key");
    fs::write(&server_cert_path, cert.cert.pem()).expect("server cert should be written");
    fs::write(&server_key_path, cert.signing_key.serialize_pem())
        .expect("server key should be written");

    let (upstream_addr, _observed_rx, upstream_task, _upstream_temp_dir) =
        spawn_h3_grpc_upstream(&server_cert_path, &server_key_path, UpstreamMode::Immediate).await;

    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-grpc-web-http3-upstream",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| {
            grpc_http3_proxy_config(listen_addr, upstream_addr, cert_path, key_path)
        },
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let response = h3_request(
        listen_addr,
        "localhost",
        "POST",
        GRPC_METHOD_PATH,
        &[(CONTENT_TYPE.as_str(), "application/grpc-web+proto"), ("x-grpc-web", "1")],
        Some(Bytes::from_static(GRPC_REQUEST_FRAME)),
        &cert.cert.pem(),
    )
    .await
    .expect("grpc-web over http3 request should succeed");

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        response.headers.get(CONTENT_TYPE.as_str()).map(String::as_str),
        Some("application/grpc-web+proto")
    );
    let (frames, trailers) = decode_grpc_web_response(response.body.as_ref());
    assert_eq!(frames, vec![Bytes::copy_from_slice(&GRPC_RESPONSE_FRAME[5..])]);
    assert_eq!(trailers.get("grpc-status").and_then(|value| value.to_str().ok()), Some("0"));
    assert_eq!(trailers.get("grpc-message").and_then(|value| value.to_str().ok()), Some("ok"));

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.await.expect("upstream h3 grpc task should finish");
}
