use super::*;

#[test]
fn upgraded_tunnels_still_count_towards_max_connections() {
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
            "unexpected upstream request line: {request:?}"
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

        let mut buffer = [0u8; 1];
        let _ = stream.read(&mut buffer);
    });

    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-upgrade-max-connections-test", |_| {
        upgrade_proxy_config_with_server_extra(
            listen_addr,
            upstream_addr,
            Some("max_connections: Some(1),"),
        )
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let deadline = Instant::now() + Duration::from_secs(1);
    let mut upgraded = loop {
        let mut candidate = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
            .expect("client should connect");
        candidate
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("client read timeout should be configurable");
        candidate
            .set_write_timeout(Some(Duration::from_secs(2)))
            .expect("client write timeout should be configurable");

        write!(
            candidate,
            "GET /ws HTTP/1.1\r\nHost: app.example.com\r\nConnection: keep-alive, Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGVzdC1rZXk=\r\n\r\n"
        )
        .expect("client should write upgrade request");
        candidate.flush().expect("upgrade request should flush");

        match try_read_http_head(&mut candidate) {
            Ok(response) => {
                assert!(
                    response.starts_with("HTTP/1.1 101"),
                    "unexpected upgrade response line: {response:?}"
                );
                break candidate;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::BrokenPipe
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::UnexpectedEof
                ) && Instant::now() < deadline =>
            {
                // The readiness probe uses the only connection slot, and the close can race
                // with the first upgrade attempt under full-suite load.
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => panic!("upgrade response should be readable: {error}"),
        }
    };

    upgraded.write_all(b"ping").expect("client should write tunneled payload");
    upgraded.flush().expect("client tunneled payload should flush");

    thread::sleep(Duration::from_millis(150));

    let mut extra = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .expect("second client should connect before being rejected");
    extra.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
    extra.set_write_timeout(Some(Duration::from_millis(500))).unwrap();
    let write_rejected = match write!(extra, "GET / HTTP/1.1\r\nHost: {listen_addr}\r\n\r\n") {
        Ok(()) => match extra.flush() {
            Ok(()) => false,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::UnexpectedEof
                ) =>
            {
                true
            }
            Err(error) => panic!("expected second connection to close cleanly, got {error}"),
        },
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::UnexpectedEof
            ) =>
        {
            true
        }
        Err(error) => panic!("expected second connection to close cleanly, got {error}"),
    };

    if write_rejected {
        drop(extra);
        drop(upgraded);
        server.shutdown_and_wait(Duration::from_secs(5));
        upstream_task.join().expect("upstream thread should complete");
        return;
    }

    let mut buffer = [0u8; 64];
    match extra.read(&mut buffer) {
        Ok(0) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::UnexpectedEof
            ) => {}
        Ok(read) => panic!(
            "expected second connection to be closed, received {:?}",
            String::from_utf8_lossy(&buffer[..read])
        ),
        Err(error) => panic!("expected second connection to close cleanly, got {error}"),
    }

    drop(extra);
    drop(upgraded);
    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.join().expect("upstream thread should complete");
}
