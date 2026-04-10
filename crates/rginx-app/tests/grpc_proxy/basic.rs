use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn proxies_basic_grpc_over_http2_with_response_trailers() {
    let (upstream_addr, observed_rx, upstream_task, upstream_temp_dir) =
        spawn_grpc_upstream().await;

    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, tls_proxy_config(listen_addr, upstream_addr));
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

    let (body_bytes, trailers) = read_body_and_trailers(response.into_body()).await;
    assert_eq!(body_bytes.freeze(), Bytes::from_static(GRPC_RESPONSE_FRAME));
    assert_eq!(
        trailers
            .as_ref()
            .and_then(|headers| headers.get("grpc-status"))
            .and_then(|value| value.to_str().ok()),
        Some("0")
    );
    assert_eq!(
        trailers
            .as_ref()
            .and_then(|headers| headers.get("grpc-message"))
            .and_then(|value| value.to_str().ok()),
        Some("ok")
    );

    let observed = tokio::time::timeout(Duration::from_secs(5), observed_rx)
        .await
        .expect("upstream request should be observed before timeout")
        .expect("upstream observation channel should complete");
    assert_eq!(observed.method, "POST");
    assert_eq!(observed.version, Version::HTTP_2);
    assert_eq!(observed.path, GRPC_METHOD_PATH);
    assert_eq!(observed.alpn_protocol.as_deref(), Some("h2"));
    assert_eq!(observed.content_type.as_deref(), Some("application/grpc"));
    assert_eq!(observed.te.as_deref(), Some("trailers"));
    assert!(observed.body.is_empty());
    assert!(observed.trailers.is_none());

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.await.expect("upstream gRPC server task should finish");
    fs::remove_dir_all(upstream_temp_dir).expect("upstream temp dir should be removed");
}

#[tokio::test(flavor = "multi_thread")]
async fn proxies_basic_grpc_over_http2_with_request_trailers() {
    let (upstream_addr, observed_rx, upstream_task, upstream_temp_dir) =
        spawn_grpc_upstream().await;

    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, tls_proxy_config(listen_addr, upstream_addr));
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
    let client: Client<_, GrpcRequestBody> = Client::builder(TokioExecutor::new()).build(connector);

    let request = Request::builder()
        .method("POST")
        .uri(format!("https://127.0.0.1:{}{GRPC_METHOD_PATH}", listen_addr.port()))
        .version(Version::HTTP_2)
        .header(CONTENT_TYPE, "application/grpc")
        .header(TE, "trailers")
        .body(GrpcRequestBody::new())
        .expect("gRPC request with trailers should build");
    let response = tokio::time::timeout(Duration::from_secs(5), client.request(request))
        .await
        .expect("gRPC request with trailers should not time out")
        .expect("gRPC request with trailers should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.version(), Version::HTTP_2);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).and_then(|value| value.to_str().ok()),
        Some("application/grpc")
    );

    let (body_bytes, trailers) = read_body_and_trailers(response.into_body()).await;
    assert_eq!(body_bytes.freeze(), Bytes::from_static(GRPC_RESPONSE_FRAME));
    assert_eq!(
        trailers
            .as_ref()
            .and_then(|headers| headers.get("grpc-status"))
            .and_then(|value| value.to_str().ok()),
        Some("0")
    );

    let observed = tokio::time::timeout(Duration::from_secs(5), observed_rx)
        .await
        .expect("upstream request should be observed before timeout")
        .expect("upstream observation channel should complete");
    assert_eq!(observed.method, "POST");
    assert_eq!(observed.version, Version::HTTP_2);
    assert_eq!(observed.path, GRPC_METHOD_PATH);
    assert_eq!(observed.alpn_protocol.as_deref(), Some("h2"));
    assert_eq!(observed.content_type.as_deref(), Some("application/grpc"));
    assert_eq!(observed.te.as_deref(), Some("trailers"));
    assert_eq!(observed.body, Bytes::from_static(GRPC_REQUEST_FRAME));
    assert_eq!(
        observed
            .trailers
            .as_ref()
            .and_then(|headers| headers.get("x-client-trailer"))
            .and_then(|value| value.to_str().ok()),
        Some("sent")
    );
    assert_eq!(
        observed
            .trailers
            .as_ref()
            .and_then(|headers| headers.get("x-request-checksum"))
            .and_then(|value| value.to_str().ok()),
        Some("abc123")
    );

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.await.expect("upstream gRPC server task should finish");
    fs::remove_dir_all(upstream_temp_dir).expect("upstream temp dir should be removed");
}

#[tokio::test(flavor = "multi_thread")]
async fn proxies_basic_grpc_web_binary_requests_to_http2_grpc_upstreams() {
    let (upstream_addr, observed_rx, upstream_task, upstream_temp_dir) =
        spawn_grpc_upstream().await;

    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(
        listen_addr,
        plain_proxy_config_with_request_timeout(listen_addr, upstream_addr, None),
    );
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
    assert!(response.headers().get("grpc-status").is_none());

    let body_bytes =
        response.into_body().collect().await.expect("grpc-web response should collect").to_bytes();
    let (frames, trailers) = decode_grpc_web_response(body_bytes.as_ref());
    assert_eq!(frames, vec![Bytes::copy_from_slice(&GRPC_RESPONSE_FRAME[5..])]);
    assert_eq!(trailers.get("grpc-status").and_then(|value| value.to_str().ok()), Some("0"));
    assert_eq!(trailers.get("grpc-message").and_then(|value| value.to_str().ok()), Some("ok"));

    let observed = tokio::time::timeout(Duration::from_secs(5), observed_rx)
        .await
        .expect("upstream request should be observed before timeout")
        .expect("upstream observation channel should complete");
    assert_eq!(observed.method, "POST");
    assert_eq!(observed.version, Version::HTTP_2);
    assert_eq!(observed.path, GRPC_METHOD_PATH);
    assert_eq!(observed.alpn_protocol.as_deref(), Some("h2"));
    assert_eq!(observed.content_type.as_deref(), Some("application/grpc+proto"));
    assert_eq!(observed.te.as_deref(), Some("trailers"));
    assert_eq!(observed.body, Bytes::from_static(GRPC_REQUEST_FRAME));
    assert!(observed.trailers.is_none());

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.await.expect("upstream gRPC server task should finish");
    fs::remove_dir_all(upstream_temp_dir).expect("upstream temp dir should be removed");
}

#[tokio::test(flavor = "multi_thread")]
async fn proxies_basic_grpc_web_text_requests_to_http2_grpc_upstreams() {
    let (upstream_addr, observed_rx, upstream_task, upstream_temp_dir) =
        spawn_grpc_upstream().await;

    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, plain_proxy_config(listen_addr, upstream_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);

    let encoded_request = format!("{}\r\n", encode_grpc_web_text_payload(GRPC_REQUEST_FRAME));
    let request = Request::builder()
        .method("POST")
        .uri(format!("http://127.0.0.1:{}{GRPC_METHOD_PATH}", listen_addr.port()))
        .version(Version::HTTP_11)
        .header(CONTENT_TYPE, "application/grpc-web-text+proto")
        .header("x-grpc-web", "1")
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
    assert!(response.headers().get("grpc-status").is_none());

    let body_bytes = response
        .into_body()
        .collect()
        .await
        .expect("grpc-web-text response should collect")
        .to_bytes();
    let decoded_body = decode_grpc_web_text_payload(body_bytes.as_ref());
    let (frames, trailers) = decode_grpc_web_response(decoded_body.as_ref());
    assert_eq!(frames, vec![Bytes::copy_from_slice(&GRPC_RESPONSE_FRAME[5..])]);
    assert_eq!(trailers.get("grpc-status").and_then(|value| value.to_str().ok()), Some("0"));
    assert_eq!(trailers.get("grpc-message").and_then(|value| value.to_str().ok()), Some("ok"));

    let observed = tokio::time::timeout(Duration::from_secs(5), observed_rx)
        .await
        .expect("upstream request should be observed before timeout")
        .expect("upstream observation channel should complete");
    assert_eq!(observed.method, "POST");
    assert_eq!(observed.version, Version::HTTP_2);
    assert_eq!(observed.path, GRPC_METHOD_PATH);
    assert_eq!(observed.alpn_protocol.as_deref(), Some("h2"));
    assert_eq!(observed.content_type.as_deref(), Some("application/grpc+proto"));
    assert_eq!(observed.te.as_deref(), Some("trailers"));
    assert_eq!(observed.body, Bytes::from_static(GRPC_REQUEST_FRAME));
    assert!(observed.trailers.is_none());

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.await.expect("upstream gRPC server task should finish");
    fs::remove_dir_all(upstream_temp_dir).expect("upstream temp dir should be removed");
}

#[tokio::test(flavor = "multi_thread")]
async fn proxies_grpc_web_binary_trailer_frames_to_http2_request_trailers() {
    let (upstream_addr, observed_rx, upstream_task, upstream_temp_dir) =
        spawn_grpc_upstream().await;

    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, plain_proxy_config(listen_addr, upstream_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);

    let request_body = grpc_web_request_with_trailers();
    let request = Request::builder()
        .method("POST")
        .uri(format!("http://127.0.0.1:{}{GRPC_METHOD_PATH}", listen_addr.port()))
        .version(Version::HTTP_11)
        .header(CONTENT_TYPE, "application/grpc-web+proto")
        .header("x-grpc-web", "1")
        .body(Full::new(request_body))
        .expect("grpc-web request with trailer frame should build");
    let response = tokio::time::timeout(Duration::from_secs(5), client.request(request))
        .await
        .expect("grpc-web request with trailer frame should not time out")
        .expect("grpc-web request with trailer frame should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes =
        response.into_body().collect().await.expect("grpc-web response should collect").to_bytes();
    let (frames, trailers) = decode_grpc_web_response(body_bytes.as_ref());
    assert_eq!(frames, vec![Bytes::copy_from_slice(&GRPC_RESPONSE_FRAME[5..])]);
    assert_eq!(trailers.get("grpc-status").and_then(|value| value.to_str().ok()), Some("0"));

    let observed = tokio::time::timeout(Duration::from_secs(5), observed_rx)
        .await
        .expect("upstream request should be observed before timeout")
        .expect("upstream observation channel should complete");
    assert_eq!(observed.content_type.as_deref(), Some("application/grpc+proto"));
    assert_eq!(observed.te.as_deref(), Some("trailers"));
    assert_eq!(observed.body, Bytes::from_static(GRPC_REQUEST_FRAME));
    assert_eq!(
        observed
            .trailers
            .as_ref()
            .and_then(|headers| headers.get("x-client-trailer"))
            .and_then(|value| value.to_str().ok()),
        Some("sent")
    );
    assert_eq!(
        observed
            .trailers
            .as_ref()
            .and_then(|headers| headers.get("x-request-checksum"))
            .and_then(|value| value.to_str().ok()),
        Some("abc123")
    );

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.await.expect("upstream gRPC server task should finish");
    fs::remove_dir_all(upstream_temp_dir).expect("upstream temp dir should be removed");
}

#[tokio::test(flavor = "multi_thread")]
async fn proxies_grpc_web_text_trailer_frames_to_http2_request_trailers() {
    let (upstream_addr, observed_rx, upstream_task, upstream_temp_dir) =
        spawn_grpc_upstream().await;

    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, plain_proxy_config(listen_addr, upstream_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);

    let request_body =
        format!("{}\n", encode_grpc_web_text_payload(grpc_web_request_with_trailers().as_ref()));
    let request = Request::builder()
        .method("POST")
        .uri(format!("http://127.0.0.1:{}{GRPC_METHOD_PATH}", listen_addr.port()))
        .version(Version::HTTP_11)
        .header(CONTENT_TYPE, "application/grpc-web-text+proto")
        .header("x-grpc-web", "1")
        .body(Full::new(Bytes::from(request_body)))
        .expect("grpc-web-text request with trailer frame should build");
    let response = tokio::time::timeout(Duration::from_secs(5), client.request(request))
        .await
        .expect("grpc-web-text request with trailer frame should not time out")
        .expect("grpc-web-text request with trailer frame should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = response
        .into_body()
        .collect()
        .await
        .expect("grpc-web-text response should collect")
        .to_bytes();
    let decoded_body = decode_grpc_web_text_payload(body_bytes.as_ref());
    let (frames, trailers) = decode_grpc_web_response(decoded_body.as_ref());
    assert_eq!(frames, vec![Bytes::copy_from_slice(&GRPC_RESPONSE_FRAME[5..])]);
    assert_eq!(trailers.get("grpc-status").and_then(|value| value.to_str().ok()), Some("0"));

    let observed = tokio::time::timeout(Duration::from_secs(5), observed_rx)
        .await
        .expect("upstream request should be observed before timeout")
        .expect("upstream observation channel should complete");
    assert_eq!(observed.content_type.as_deref(), Some("application/grpc+proto"));
    assert_eq!(observed.te.as_deref(), Some("trailers"));
    assert_eq!(observed.body, Bytes::from_static(GRPC_REQUEST_FRAME));
    assert_eq!(
        observed
            .trailers
            .as_ref()
            .and_then(|headers| headers.get("x-client-trailer"))
            .and_then(|value| value.to_str().ok()),
        Some("sent")
    );
    assert_eq!(
        observed
            .trailers
            .as_ref()
            .and_then(|headers| headers.get("x-request-checksum"))
            .and_then(|value| value.to_str().ok()),
        Some("abc123")
    );

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.await.expect("upstream gRPC server task should finish");
    fs::remove_dir_all(upstream_temp_dir).expect("upstream temp dir should be removed");
}

#[tokio::test(flavor = "multi_thread")]
async fn routes_grpc_requests_by_service_and_method() {
    let (service_addr, service_rx, service_task, service_temp_dir) = spawn_grpc_upstream().await;
    let (method_addr, method_rx, method_task, method_temp_dir) = spawn_grpc_upstream().await;

    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(
        listen_addr,
        plain_grpc_service_method_routing_config(listen_addr, service_addr, method_addr),
    );
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
    let body_bytes =
        response.into_body().collect().await.expect("grpc-web response should collect").to_bytes();
    let (frames, trailers) = decode_grpc_web_response(body_bytes.as_ref());
    assert_eq!(frames, vec![Bytes::copy_from_slice(&GRPC_RESPONSE_FRAME[5..])]);
    assert_eq!(trailers.get("grpc-status").and_then(|value| value.to_str().ok()), Some("0"));

    let observed = tokio::time::timeout(Duration::from_secs(5), method_rx)
        .await
        .expect("method-specific upstream should receive the request before timeout")
        .expect("method-specific upstream observation channel should complete");
    assert_eq!(observed.path, GRPC_METHOD_PATH);
    assert_eq!(observed.content_type.as_deref(), Some("application/grpc+proto"));

    assert!(
        tokio::time::timeout(Duration::from_millis(300), service_rx).await.is_err(),
        "service-level fallback upstream should not be selected for a method-specific match"
    );

    server.shutdown_and_wait(Duration::from_secs(5));
    method_task.await.expect("method-specific upstream task should finish");
    service_task.abort();
    let _ = service_task.await;
    fs::remove_dir_all(method_temp_dir).expect("method-specific temp dir should be removed");
    fs::remove_dir_all(service_temp_dir).expect("service-level temp dir should be removed");
}
