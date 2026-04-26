use super::*;

#[test]
fn reload_keeps_upgraded_tunnel_alive() {
    let upstream_listener =
        TcpListener::bind(("127.0.0.1", 0)).expect("upstream listener should bind");
    let upstream_addr =
        upstream_listener.local_addr().expect("upstream listener addr should be available");
    let upstream_task = thread::spawn(move || {
        let (mut stream, _) = upstream_listener.accept().expect("upstream should accept a client");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("upstream read timeout should be configurable");
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .expect("upstream write timeout should be configurable");

        let request = read_http_head(&mut stream);
        assert!(
            request.starts_with("GET /ws HTTP/1.1\r\n"),
            "unexpected upstream request: {request:?}"
        );

        stream
            .write_all(
                b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n",
            )
            .expect("upstream should write switching protocols response");
        stream.flush().expect("upstream response should flush");

        let mut first = [0u8; 4];
        stream.read_exact(&mut first).expect("upstream should read first tunneled payload");
        assert_eq!(&first, b"ping");
        stream.write_all(b"pong").expect("upstream should write first tunneled response payload");
        stream.flush().expect("first tunneled response should flush");

        let mut second = [0u8; 4];
        stream.read_exact(&mut second).expect("upstream should read second tunneled payload");
        assert_eq!(&second, b"more");
        stream.write_all(b"done").expect("upstream should write second tunneled response payload");
        stream.flush().expect("second tunneled response should flush");
    });

    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-upgrade-reload-test", |_| {
        upgrade_proxy_config_with_status_body(listen_addr, upstream_addr, "before reload\n")
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));
    server.wait_for_http_text_response(
        listen_addr,
        &listen_addr.to_string(),
        "/status",
        200,
        "before reload\n",
        Duration::from_secs(5),
    );

    let mut client = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .expect("client should connect");
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("client read timeout should be configurable");
    client
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("client write timeout should be configurable");

    write!(
        client,
        "GET /ws HTTP/1.1\r\nHost: app.example.com\r\nConnection: keep-alive, Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGVzdC1rZXk=\r\n\r\n"
    )
    .expect("client should write upgrade request");
    client.flush().expect("upgrade request should flush");
    let response = read_http_head(&mut client);
    assert!(response.starts_with("HTTP/1.1 101"), "unexpected upgrade response: {response:?}");

    client.write_all(b"ping").expect("client should write first tunneled payload");
    client.flush().expect("first tunneled payload should flush");
    let mut payload = [0u8; 4];
    client.read_exact(&mut payload).expect("client should read first tunneled response payload");
    assert_eq!(&payload, b"pong");

    std::fs::write(
        server.config_path(),
        upgrade_proxy_config_with_status_body(listen_addr, upstream_addr, "after reload\n"),
    )
    .expect("reloaded config should be written");
    server.send_signal(libc::SIGHUP);
    server.wait_for_http_text_response(
        listen_addr,
        &listen_addr.to_string(),
        "/status",
        200,
        "after reload\n",
        Duration::from_secs(5),
    );

    client.write_all(b"more").expect("client should write second tunneled payload");
    client.flush().expect("second tunneled payload should flush");
    client.read_exact(&mut payload).expect("client should read second tunneled response payload");
    assert_eq!(&payload, b"done");

    drop(client);
    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.join().expect("upstream thread should complete");
}
