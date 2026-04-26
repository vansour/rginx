use super::*;

#[test]
fn header_read_timeout_closes_slow_request_connections() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(
        listen_addr,
        return_config(listen_addr, Some("header_read_timeout_secs: Some(1),"), "ok\n"),
    );

    server.wait_for_body(listen_addr, "/", "ok\n", Duration::from_secs(5));

    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .expect("slow client should connect");
    stream.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
    stream.set_write_timeout(Some(Duration::from_millis(500))).unwrap();
    write!(stream, "GET / HTTP/1.1\r\nHost: {listen_addr}\r\n").unwrap();
    stream.flush().unwrap();

    thread::sleep(Duration::from_millis(1_500));

    assert_connection_closed(&mut stream, Some(b"Connection: close\r\n\r\n"));
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn max_connections_rejects_new_connections_when_limit_is_reached() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(
        listen_addr,
        return_config(listen_addr, Some("max_connections: Some(1),"), "ok\n"),
    );

    server.wait_for_body(listen_addr, "/", "ok\n", Duration::from_secs(5));

    let mut held = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .expect("first client should connect");
    held.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
    held.set_write_timeout(Some(Duration::from_millis(500))).unwrap();
    write!(held, "GET / HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: keep-alive\r\n\r\n")
        .unwrap();
    held.flush().unwrap();

    let response = read_http_response_once(&mut held).expect("first connection should succeed");
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.ends_with("ok\n"));

    let mut extra = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .expect("second client should connect before being rejected");
    extra.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
    extra.set_write_timeout(Some(Duration::from_millis(500))).unwrap();

    assert_connection_closed(&mut extra, Some(request_bytes(listen_addr, "/").as_bytes()));
    drop(held);
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn keep_alive_disabled_closes_connections_after_each_response() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(
        listen_addr,
        return_config(listen_addr, Some("keep_alive: Some(false),"), "ok\n"),
    );

    server.wait_for_body(listen_addr, "/", "ok\n", Duration::from_secs(5));

    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .expect("client should connect");
    stream.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
    stream.set_write_timeout(Some(Duration::from_millis(500))).unwrap();
    write!(stream, "GET / HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: keep-alive\r\n\r\n")
        .unwrap();
    stream.flush().unwrap();

    let response = read_http_response_once(&mut stream).expect("first response should be readable");
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.ends_with("ok\n"));

    assert_connection_closed(&mut stream, Some(request_bytes(listen_addr, "/").as_bytes()));
    server.shutdown_and_wait(Duration::from_secs(5));
}
