use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn rejects_invalid_grpc_timeout_for_grpc_web_requests() {
    let (upstream_addr, observed_rx, upstream_task, upstream_temp_dir) =
        spawn_grpc_upstream().await;

    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, plain_proxy_config(listen_addr, upstream_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);

    let request = Request::builder()
        .method("POST")
        .uri(format!("http://127.0.0.1:{}{GRPC_METHOD_PATH}", listen_addr.port()))
        .version(Version::HTTP_11)
        .header(CONTENT_TYPE, "application/grpc-web+proto")
        .header("x-grpc-web", "1")
        .header("grpc-timeout", "soon")
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
        Some("3")
    );
    assert!(
        response
            .headers()
            .get("grpc-message")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|message| message.contains("invalid grpc-timeout header"))
    );

    let body_bytes =
        response.into_body().collect().await.expect("grpc-web response should collect").to_bytes();
    let (frames, trailers) = decode_grpc_web_response(body_bytes.as_ref());
    assert!(frames.is_empty());
    assert_eq!(trailers.get("grpc-status").and_then(|value| value.to_str().ok()), Some("3"));
    assert!(
        trailers
            .get("grpc-message")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|message| message.contains("invalid grpc-timeout header"))
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(300), observed_rx).await.is_err(),
        "invalid grpc-timeout should be rejected before contacting upstream"
    );

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.abort();
    let _ = upstream_task.await;
    fs::remove_dir_all(upstream_temp_dir).expect("upstream temp dir should be removed");
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_invalid_grpc_web_text_request_body_before_contacting_upstream() {
    let upstream_addr = reserve_loopback_addr();
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(
        listen_addr,
        plain_proxy_config_with_server_extra(
            listen_addr,
            upstream_addr,
            "        max_request_body_bytes: Some(1024),\n",
        ),
    );
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);

    let request = Request::builder()
        .method("POST")
        .uri(format!("http://127.0.0.1:{}{GRPC_METHOD_PATH}", listen_addr.port()))
        .version(Version::HTTP_11)
        .header(CONTENT_TYPE, "application/grpc-web-text+proto")
        .header("x-grpc-web", "1")
        .body(Full::new(Bytes::from_static(b"%%%")))
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
    assert_eq!(
        response.headers().get("grpc-status").and_then(|value| value.to_str().ok()),
        Some("3")
    );
    assert!(
        response
            .headers()
            .get("grpc-message")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|message| message.contains("invalid downstream request body"))
    );

    let body_bytes = response
        .into_body()
        .collect()
        .await
        .expect("grpc-web-text response should collect")
        .to_bytes();
    let decoded_body = STANDARD
        .decode(body_bytes.as_ref())
        .expect("grpc-web-text error body should be valid base64");
    let (frames, trailers) = decode_grpc_web_response(decoded_body.as_ref());
    assert!(frames.is_empty());
    assert_eq!(trailers.get("grpc-status").and_then(|value| value.to_str().ok()), Some("3"));
    assert!(
        trailers
            .get("grpc-message")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|message| message.contains("invalid downstream request body"))
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}
