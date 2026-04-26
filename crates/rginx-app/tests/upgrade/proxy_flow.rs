use super::*;

#[test]
fn proxies_http_upgrade_streams_end_to_end() {
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
        let request_lower = request.to_ascii_lowercase();
        assert!(
            request.starts_with("GET /ws HTTP/1.1\r\n"),
            "unexpected upstream request line: {request:?}"
        );
        assert!(
            request_lower.contains("\r\nconnection: upgrade\r\n"),
            "upgrade connection header should be preserved: {request:?}"
        );
        assert!(
            request_lower.contains("\r\nupgrade: websocket\r\n"),
            "upgrade protocol header should be preserved: {request:?}"
        );

        stream
            .write_all(
                b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n",
            )
            .expect("upstream should write switching protocols response");
        stream.flush().expect("upstream response should flush");

        let mut payload = [0u8; 4];
        stream.read_exact(&mut payload).expect("upstream should read tunneled payload");
        assert_eq!(&payload, b"ping");

        stream.write_all(b"pong").expect("upstream should write tunneled response payload");
        stream.flush().expect("upstream tunneled response should flush");
    });

    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-upgrade-test", |_| {
        upgrade_proxy_config(listen_addr, upstream_addr)
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));
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
    let response_lower = response.to_ascii_lowercase();
    assert!(response.starts_with("HTTP/1.1 101"), "unexpected upgrade response line: {response:?}");
    assert!(
        response_lower.contains("\r\nconnection: upgrade\r\n"),
        "upgrade response should preserve connection header: {response:?}"
    );
    assert!(
        response_lower.contains("\r\nupgrade: websocket\r\n"),
        "upgrade response should preserve protocol header: {response:?}"
    );

    client.write_all(b"ping").expect("client should write tunneled payload");
    client.flush().expect("client tunneled payload should flush");

    let mut payload = [0u8; 4];
    client.read_exact(&mut payload).expect("client should read tunneled response payload");
    assert_eq!(&payload, b"pong");

    drop(client);
    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.join().expect("upstream thread should complete");
}
