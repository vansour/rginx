use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn returns_grpc_status_for_unmatched_http2_grpc_requests() {
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, tls_unmatched_grpc_config(listen_addr));
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let connector = HttpsConnectorBuilder::new()
        .with_tls_config(
            ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(InsecureServerCertVerifier::new()))
                .with_no_client_auth(),
        )
        .https_only()
        .enable_http2()
        .build();
    let client: Client<_, Empty<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);

    let request = Request::builder()
        .method("POST")
        .uri(format!("https://127.0.0.1:{}{GRPC_METHOD_PATH}", listen_addr.port()))
        .version(Version::HTTP_2)
        .header(CONTENT_TYPE, "application/grpc")
        .header(TE, "trailers")
        .body(Empty::<Bytes>::new())
        .expect("gRPC request should build");
    let response = tokio::time::timeout(Duration::from_secs(5), client.request(request))
        .await
        .expect("gRPC request should not time out")
        .expect("gRPC request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.version(), Version::HTTP_2);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).and_then(|value| value.to_str().ok()),
        Some("application/grpc")
    );
    assert_eq!(
        response.headers().get("grpc-status").and_then(|value| value.to_str().ok()),
        Some("12")
    );
    assert_eq!(
        response.headers().get("grpc-message").and_then(|value| value.to_str().ok()),
        Some("route not found")
    );
    let body = response.into_body().collect().await.expect("body should collect").to_bytes();
    assert!(body.is_empty());

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread")]
async fn returns_grpc_web_status_for_unavailable_upstreams() {
    let unavailable_addr = reserve_loopback_addr();
    let listen_addr = reserve_loopback_addr();
    let mut server =
        TestServer::spawn(listen_addr, plain_proxy_config(listen_addr, unavailable_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);

    let request = Request::builder()
        .method("POST")
        .uri(format!("http://127.0.0.1:{}{GRPC_METHOD_PATH}", listen_addr.port()))
        .version(Version::HTTP_11)
        .header(CONTENT_TYPE, "application/grpc-web+proto")
        .header("x-grpc-web", "1")
        .body(Full::new(Bytes::from_static(GRPC_REQUEST_FRAME)))
        .expect("grpc-web request should build");
    let response = tokio::time::timeout(Duration::from_secs(5), client.request(request))
        .await
        .expect("grpc-web request should not time out")
        .expect("grpc-web request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.version(), Version::HTTP_11);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).and_then(|value| value.to_str().ok()),
        Some("application/grpc-web+proto")
    );
    assert_eq!(
        response.headers().get("grpc-status").and_then(|value| value.to_str().ok()),
        Some("14")
    );

    let body_bytes =
        response.into_body().collect().await.expect("grpc-web response should collect").to_bytes();
    let (frames, trailers) = decode_grpc_web_response(body_bytes.as_ref());
    assert!(frames.is_empty());
    assert_eq!(trailers.get("grpc-status").and_then(|value| value.to_str().ok()), Some("14"));
    assert_eq!(
        trailers.get("grpc-message").and_then(|value| value.to_str().ok()),
        Some("upstream `backend` is unavailable")
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread")]
async fn active_grpc_health_checks_gate_proxy_requests_until_peer_recovers() {
    // Start upstream as NOT_SERVING (status 0). Health checks will fail.
    let health_status = Arc::new(AtomicU8::new(0));
    let (upstream_addr, upstream_shutdown_tx, upstream_task, upstream_temp_dir) =
        spawn_grpc_upstream_with_dynamic_health(health_status.clone()).await;

    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(
        listen_addr,
        plain_proxy_config_with_grpc_health_check(listen_addr, upstream_addr),
    );
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);

    // Wait for the health check to fail and peer to enter cooldown.
    // health_check_interval_secs=1, healthy_successes_required=2
    // The peer starts unhealthy, so it will be skipped immediately.
    // After ~1s cooldown, another probe will fail and it enters cooldown.
    tokio::time::sleep(Duration::from_secs(3)).await;

    // At this point peer should be in cooldown/unhealthy, proxy should return error.
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut got_unhealthy = false;
    while Instant::now() < deadline {
        let request = Request::builder()
            .method("POST")
            .uri(format!("http://127.0.0.1:{}{APP_GRPC_METHOD_PATH}", listen_addr.port()))
            .version(Version::HTTP_11)
            .header(CONTENT_TYPE, "application/grpc-web+proto")
            .header("x-grpc-web", "1")
            .body(Full::new(Bytes::from_static(GRPC_REQUEST_FRAME)))
            .expect("grpc-web request should build");
        let response = client.request(request).await.expect("grpc-web request should succeed");
        let body_bytes = response
            .into_body()
            .collect()
            .await
            .expect("grpc-web response should collect")
            .to_bytes();
        let (_, trailers) = decode_grpc_web_response(body_bytes.as_ref());
        if trailers.get("grpc-status").and_then(|v| v.to_str().ok()) == Some("14") {
            got_unhealthy = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(got_unhealthy, "expected grpc-status=14 (no healthy peers)");

    // Recover: set health_status to SERVING.
    health_status.store(1, Ordering::Relaxed);

    // Wait for recovery: healthy_successes_required=2, interval=1s => ~3s
    // Health check probes must succeed 2 times before the peer is recovered.
    // After cooldown + 2 successful probes, peer should be healthy again.
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut got_ok = false;
    while Instant::now() < deadline {
        let request = Request::builder()
            .method("POST")
            .uri(format!("http://127.0.0.1:{}{APP_GRPC_METHOD_PATH}", listen_addr.port()))
            .version(Version::HTTP_11)
            .header(CONTENT_TYPE, "application/grpc-web+proto")
            .header("x-grpc-web", "1")
            .body(Full::new(Bytes::from_static(GRPC_REQUEST_FRAME)))
            .expect("grpc-web request should build");
        let response = client.request(request).await.expect("grpc-web request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = response
            .into_body()
            .collect()
            .await
            .expect("grpc-web response should collect")
            .to_bytes();
        let (_frames, trailers) = decode_grpc_web_response(body_bytes.as_ref());
        if trailers.get("grpc-status").and_then(|v| v.to_str().ok()) == Some("0") {
            got_ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(got_ok, "expected grpc-status=0 after peer recovers");

    server.shutdown_and_wait(Duration::from_secs(5));
    let _ = upstream_shutdown_tx.send(());
    upstream_task.await.expect("upstream gRPC server task should finish");
    fs::remove_dir_all(upstream_temp_dir).expect("upstream temp dir should be removed");
}

#[tokio::test(flavor = "multi_thread")]
async fn downstream_cancellation_closes_upstream_grpc_stream_and_records_status_1() {
    let (upstream_addr, upstream_cancelled_rx, upstream_task, upstream_temp_dir) =
        spawn_cancellable_grpc_upstream().await;

    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, tls_proxy_config(listen_addr, upstream_addr));
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let client: Client<_, Empty<Bytes>> =
        Client::builder(TokioExecutor::new()).build(https_h2_connector());

    let request = Request::builder()
        .method("POST")
        .uri(format!("https://127.0.0.1:{}{APP_GRPC_METHOD_PATH}", listen_addr.port()))
        .version(Version::HTTP_2)
        .header(CONTENT_TYPE, "application/grpc")
        .header(TE, "trailers")
        .body(Empty::<Bytes>::new())
        .expect("gRPC request should build");
    let response = tokio::time::timeout(Duration::from_secs(5), client.request(request))
        .await
        .expect("gRPC request should not time out")
        .expect("gRPC request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.version(), Version::HTTP_2);

    let mut body = response.into_body();
    let first_frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
        .await
        .expect("first response frame should arrive before timeout")
        .expect("response body should yield a frame")
        .expect("first response frame should be successful");
    assert_eq!(
        first_frame.into_data().expect("first frame should be response data"),
        Bytes::from_static(GRPC_RESPONSE_FRAME)
    );

    drop(body);

    tokio::time::timeout(Duration::from_secs(2), upstream_cancelled_rx)
        .await
        .expect("upstream response stream should be cancelled before timeout")
        .expect("upstream cancellation notification should arrive");

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.await.expect("upstream gRPC server task should finish");
    fs::remove_dir_all(upstream_temp_dir).expect("upstream temp dir should be removed");
}

#[tokio::test(flavor = "multi_thread")]
async fn grpc_web_cancellation_closes_upstream_stream_and_emits_access_log_status_1() {
    let (upstream_addr, upstream_cancelled_rx, upstream_task, upstream_temp_dir) =
        spawn_cancellable_grpc_upstream().await;

    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(
        listen_addr,
        plain_proxy_config_with_access_log(listen_addr, upstream_addr),
    );
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);

    let request = Request::builder()
        .method("POST")
        .uri(format!("http://127.0.0.1:{}{APP_GRPC_METHOD_PATH}", listen_addr.port()))
        .version(Version::HTTP_11)
        .header(CONTENT_TYPE, "application/grpc-web+proto")
        .header("x-grpc-web", "1")
        .header("x-request-id", "grpc-web-cancel-1")
        .body(Full::new(Bytes::from_static(GRPC_REQUEST_FRAME)))
        .expect("grpc-web request should build");
    let response = tokio::time::timeout(Duration::from_secs(5), client.request(request))
        .await
        .expect("grpc-web request should not time out")
        .expect("grpc-web request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.version(), Version::HTTP_11);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).and_then(|value| value.to_str().ok()),
        Some("application/grpc-web+proto")
    );

    let mut body = response.into_body();
    let first_frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
        .await
        .expect("first grpc-web frame should arrive before timeout")
        .expect("grpc-web body should yield a frame")
        .expect("first grpc-web frame should be successful");
    assert_eq!(
        first_frame.into_data().expect("first grpc-web frame should be data"),
        Bytes::from_static(GRPC_RESPONSE_FRAME)
    );

    drop(body);

    tokio::time::timeout(Duration::from_secs(2), upstream_cancelled_rx)
        .await
        .expect("upstream grpc-web response stream should be cancelled before timeout")
        .expect("upstream cancellation notification should arrive");

    wait_for_log_contains(
        &server,
        Duration::from_secs(5),
        "ACCESS reqid=grpc-web-cancel-1 grpc=grpc-web svc=demo.Test rpc=Ping grpc_status=1 grpc_message=\"downstream cancelled\"",
    )
    .await;

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.await.expect("upstream gRPC server task should finish");
    fs::remove_dir_all(upstream_temp_dir).expect("upstream temp dir should be removed");
}

#[tokio::test(flavor = "multi_thread")]
async fn grpc_web_text_cancellation_closes_upstream_stream_and_emits_access_log_status_1() {
    let (upstream_addr, upstream_cancelled_rx, upstream_task, upstream_temp_dir) =
        spawn_cancellable_grpc_upstream().await;

    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(
        listen_addr,
        plain_proxy_config_with_access_log(listen_addr, upstream_addr),
    );
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);

    let encoded_request = format!("{}\r\n", encode_grpc_web_text_payload(GRPC_REQUEST_FRAME));
    let request = Request::builder()
        .method("POST")
        .uri(format!("http://127.0.0.1:{}{APP_GRPC_METHOD_PATH}", listen_addr.port()))
        .version(Version::HTTP_11)
        .header(CONTENT_TYPE, "application/grpc-web-text+proto")
        .header("x-grpc-web", "1")
        .header("x-request-id", "grpc-web-text-cancel-1")
        .body(Full::new(Bytes::from(encoded_request)))
        .expect("grpc-web-text request should build");
    let response = tokio::time::timeout(Duration::from_secs(5), client.request(request))
        .await
        .expect("grpc-web-text request should not time out")
        .expect("grpc-web-text request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.version(), Version::HTTP_11);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).and_then(|value| value.to_str().ok()),
        Some("application/grpc-web-text+proto")
    );

    let mut body = response.into_body();
    let first_frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
        .await
        .expect("first grpc-web-text frame should arrive before timeout")
        .expect("grpc-web-text body should yield a frame")
        .expect("first grpc-web-text frame should be successful");
    assert!(!first_frame.into_data().expect("first grpc-web-text frame should be data").is_empty());

    drop(body);

    tokio::time::timeout(Duration::from_secs(2), upstream_cancelled_rx)
        .await
        .expect("upstream grpc-web-text response stream should be cancelled before timeout")
        .expect("upstream cancellation notification should arrive");

    wait_for_log_contains(
        &server,
        Duration::from_secs(5),
        "ACCESS reqid=grpc-web-text-cancel-1 grpc=grpc-web-text svc=demo.Test rpc=Ping grpc_status=1 grpc_message=\"downstream cancelled\"",
    )
    .await;

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.await.expect("upstream gRPC server task should finish");
    fs::remove_dir_all(upstream_temp_dir).expect("upstream temp dir should be removed");
}
