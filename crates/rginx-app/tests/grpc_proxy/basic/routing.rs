use super::*;

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
