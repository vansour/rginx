use super::*;

#[test]
fn max_headers_rejects_requests_with_too_many_headers() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(
        listen_addr,
        return_config(listen_addr, Some("max_headers: Some(2),"), "ok\n"),
    );

    server.wait_for_body(listen_addr, "/", "ok\n", Duration::from_secs(5));

    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .expect("client should connect");
    stream.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
    stream.set_write_timeout(Some(Duration::from_millis(500))).unwrap();
    write!(
        stream,
        "GET / HTTP/1.1\r\nHost: {listen_addr}\r\nX-Test: 1\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    stream.flush().unwrap();

    let response = read_http_response_once(&mut stream).expect("response should be readable");
    assert!(
        response.starts_with("HTTP/1.1 431"),
        "expected 431 for header overflow, got {response:?}"
    );
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn max_request_body_bytes_rejects_chunked_proxy_requests_that_exceed_limit() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let upstream_addr = spawn_response_server("backend ok\n");
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(
        listen_addr,
        proxy_config(listen_addr, upstream_addr, Some("max_request_body_bytes: Some(8),")),
    );

    server.wait_for_ready(listen_addr, Duration::from_secs(5));

    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .expect("client should connect");
    stream.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
    stream.set_write_timeout(Some(Duration::from_millis(500))).unwrap();
    write!(
        stream,
        "POST /api/upload HTTP/1.1\r\nHost: {listen_addr}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n5\r\nworld\r\n0\r\n\r\n"
    )
    .unwrap();
    stream.flush().unwrap();

    let response = read_http_response_once(&mut stream).expect("response should be readable");
    let (status, _body) = parse_response(&response).expect("response should be valid HTTP");
    assert_eq!(status, 413, "expected 413 for oversized chunked body, got {response:?}");
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn request_body_read_timeout_rejects_stalled_proxy_uploads() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let upstream_addr = spawn_drain_request_server();
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(
        listen_addr,
        proxy_config_with_upstream_extra(
            listen_addr,
            upstream_addr,
            Some("request_body_read_timeout_secs: Some(1),"),
            Some(
                "request_timeout_secs: Some(5),\n            unhealthy_after_failures: Some(2),\n            unhealthy_cooldown_secs: Some(1),",
            ),
        ),
    );

    server.wait_for_ready(listen_addr, Duration::from_secs(5));

    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .expect("client should connect");
    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    stream.set_write_timeout(Some(Duration::from_millis(500))).unwrap();
    write!(
        stream,
        "POST /api/upload HTTP/1.1\r\nHost: {listen_addr}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n"
    )
    .unwrap();
    stream.flush().unwrap();

    thread::sleep(Duration::from_millis(1_500));

    let response = read_http_response_bytes(&mut stream).expect("response should be readable");
    let parsed = parse_http_response(&response).expect("response should be valid HTTP");
    assert_eq!(parsed.status, 502, "expected 502 for timed out upload, got {parsed:?}");
    assert!(
        String::from_utf8_lossy(&parsed.body).contains("upstream `backend` is unavailable"),
        "unexpected timeout response body: {:?}",
        String::from_utf8_lossy(&parsed.body)
    );
    server.shutdown_and_wait(Duration::from_secs(5));
}
