use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn serves_return_handler_over_http3() {
    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-return",
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
    assert_eq!(body_text(&response), "http3 return\n");

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread")]
async fn proxies_basic_http_requests_over_http3_to_http11_upstreams() {
    let upstream_listener =
        TcpListener::bind(("127.0.0.1", 0)).expect("upstream listener should bind");
    upstream_listener
        .set_nonblocking(false)
        .expect("upstream listener should support blocking mode");
    let upstream_addr = upstream_listener.local_addr().expect("upstream addr should be available");
    let upstream_task = thread::spawn(move || {
        let (mut stream, _) =
            upstream_listener.accept().expect("upstream connection should arrive");
        let request = read_http_head_from_stream(&mut stream);
        assert!(
            request.starts_with("GET /demo HTTP/1.1\r\n"),
            "unexpected upstream request: {request}"
        );
        let body = "http3 proxy ok\n";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("upstream response should write");
        stream.flush().expect("upstream response should flush");
    });

    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-proxy",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| {
            http3_proxy_config(listen_addr, upstream_addr, cert_path, key_path)
        },
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let response = http3_get(listen_addr, "localhost", "/api/demo", &cert.cert.pem())
        .await
        .expect("http3 proxy request should succeed");

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(body_text(&response), "http3 proxy ok\n");

    upstream_task.join().expect("upstream task should complete");
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread")]
async fn streams_http3_responses_without_buffering_entire_upstream_body() {
    let upstream_listener =
        TcpListener::bind(("127.0.0.1", 0)).expect("upstream listener should bind");
    upstream_listener
        .set_nonblocking(false)
        .expect("upstream listener should support blocking mode");
    let upstream_addr = upstream_listener.local_addr().expect("upstream addr should be available");
    let upstream_task = thread::spawn(move || {
        let (mut stream, _) =
            upstream_listener.accept().expect("upstream connection should arrive");
        let request = read_http_head_from_stream(&mut stream);
        assert!(
            request.starts_with("GET /stream HTTP/1.1\r\n"),
            "unexpected upstream request: {request}"
        );
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
        )
        .expect("upstream response head should write");
        stream.flush().expect("upstream response head should flush");
        write_chunked_payload(&mut stream, b"first\n");
        thread::sleep(Duration::from_millis(900));
        write_chunked_payload(&mut stream, b"second\n");
        stream.write_all(b"0\r\n\r\n").expect("terminal chunk should write");
        stream.flush().expect("terminal chunk should flush");
    });

    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-streaming",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| {
            http3_streaming_proxy_config(listen_addr, upstream_addr, cert_path, key_path)
        },
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let (status, first, second) =
        http3_streaming_get_two_chunks(listen_addr, "localhost", "/api/stream", &cert.cert.pem())
            .await
            .expect("http3 streaming request should succeed");

    assert_eq!(status, StatusCode::OK);
    assert_eq!(&first[..], b"first\n");
    assert_eq!(&second[..], b"second\n");

    upstream_task.join().expect("upstream task should complete");
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread")]
async fn tls_responses_advertise_alt_svc_when_http3_is_enabled() {
    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-alt-svc",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| http3_return_config(listen_addr, cert_path, key_path),
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let client = https_client(&cert.cert.pem());
    let request = Request::builder()
        .method("GET")
        .uri(format!("https://localhost:{}/v3", listen_addr.port()))
        .body(Empty::<Bytes>::new())
        .expect("https request should build");
    let response = client.request(request).await.expect("https request should succeed");
    let expected_alt_svc = format!("h3=\":{}\"; ma=7200", listen_addr.port());

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(hyper::http::header::ALT_SVC).and_then(|value| value.to_str().ok()),
        Some(expected_alt_svc.as_str())
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}
