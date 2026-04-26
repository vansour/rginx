use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn respects_grpc_timeout_for_http2_grpc_requests() {
    let (upstream_addr, observed_rx, upstream_task, upstream_temp_dir) =
        spawn_grpc_upstream_with_response_delay(Duration::from_secs(3)).await;

    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(
        listen_addr,
        tls_proxy_config_with_request_timeout(listen_addr, upstream_addr, Some(2)),
    );
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

    let started = Instant::now();
    let request = Request::builder()
        .method("POST")
        .uri(format!("https://127.0.0.1:{}{GRPC_METHOD_PATH}", listen_addr.port()))
        .version(Version::HTTP_2)
        .header(CONTENT_TYPE, "application/grpc")
        .header(TE, "trailers")
        .header("grpc-timeout", "200m")
        .body(Empty::<Bytes>::new())
        .expect("gRPC request should build");
    let response = tokio::time::timeout(Duration::from_secs(5), client.request(request))
        .await
        .expect("gRPC request should not time out")
        .expect("gRPC request should succeed");
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(1),
        "grpc-timeout should win over upstream timeout, elapsed={elapsed:?}"
    );
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.version(), Version::HTTP_2);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).and_then(|value| value.to_str().ok()),
        Some("application/grpc")
    );
    assert_eq!(
        response.headers().get("grpc-status").and_then(|value| value.to_str().ok()),
        Some("4")
    );
    assert!(
        response
            .headers()
            .get("grpc-message")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|message| message.contains("timed out after 200 ms"))
    );
    let body = response.into_body().collect().await.expect("body should collect").to_bytes();
    assert!(body.is_empty());

    let observed = tokio::time::timeout(Duration::from_secs(5), observed_rx)
        .await
        .expect("upstream request should be observed before timeout")
        .expect("upstream observation channel should complete");
    assert_eq!(observed.grpc_timeout.as_deref(), Some("200m"));

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.abort();
    let _ = upstream_task.await;
    fs::remove_dir_all(upstream_temp_dir).expect("upstream temp dir should be removed");
}

#[tokio::test(flavor = "multi_thread")]
async fn respects_grpc_timeout_across_http2_grpc_response_body_streams() {
    let (upstream_addr, observed_rx, upstream_task, upstream_temp_dir) =
        spawn_grpc_upstream_with_body_delay(Duration::from_secs(3)).await;

    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(
        listen_addr,
        tls_proxy_config_with_request_timeout(listen_addr, upstream_addr, Some(2)),
    );
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
        .header("grpc-timeout", "200m")
        .body(Empty::<Bytes>::new())
        .expect("gRPC request should build");

    let started = Instant::now();
    let response = tokio::time::timeout(Duration::from_secs(5), client.request(request))
        .await
        .expect("gRPC request should not time out")
        .expect("gRPC request should succeed");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "response headers should arrive before the response body deadline kicks in"
    );

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).and_then(|value| value.to_str().ok()),
        Some("application/grpc")
    );

    let body_started = Instant::now();
    let (body_bytes, trailers) = read_body_and_trailers(response.into_body()).await;
    assert!(
        body_started.elapsed() < Duration::from_secs(1),
        "response body should be cut off by grpc-timeout before the upstream body delay finishes"
    );
    assert!(body_bytes.is_empty());
    assert_eq!(
        trailers
            .as_ref()
            .and_then(|headers| headers.get("grpc-status"))
            .and_then(|value| value.to_str().ok()),
        Some("4")
    );
    assert!(
        trailers
            .as_ref()
            .and_then(|headers| headers.get("grpc-message"))
            .and_then(|value| value.to_str().ok())
            .is_some_and(|message| message.contains("timed out after 200 ms"))
    );

    let observed = tokio::time::timeout(Duration::from_secs(5), observed_rx)
        .await
        .expect("upstream request should be observed before timeout")
        .expect("upstream observation channel should complete");
    assert_eq!(observed.grpc_timeout.as_deref(), Some("200m"));

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.abort();
    let _ = upstream_task.await;
    fs::remove_dir_all(upstream_temp_dir).expect("upstream temp dir should be removed");
}
