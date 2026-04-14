use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, read_http_head, reserve_loopback_addr};

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

    let mut upgraded = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .expect("client should connect");
    upgraded
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("client read timeout should be configurable");
    upgraded
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("client write timeout should be configurable");

    write!(
        upgraded,
        "GET /ws HTTP/1.1\r\nHost: app.example.com\r\nConnection: keep-alive, Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGVzdC1rZXk=\r\n\r\n"
    )
    .expect("client should write upgrade request");
    upgraded.flush().expect("upgrade request should flush");

    let response = read_http_head(&mut upgraded);
    assert!(response.starts_with("HTTP/1.1 101"), "unexpected upgrade response line: {response:?}");

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

#[cfg(unix)]
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

fn upgrade_proxy_config(listen_addr: SocketAddr, upstream_addr: SocketAddr) -> String {
    upgrade_proxy_config_with_server_extra(listen_addr, upstream_addr, None)
}

fn upgrade_proxy_config_with_status_body(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    status_body: &str,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/status\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some({status_body:?}),\n            ),\n        ),\n        LocationConfig(\n            matcher: Prefix(\"/ws\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("http://{upstream_addr}"),
        status_body = status_body,
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn upgrade_proxy_config_with_server_extra(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    server_extra: Option<&str>,
) -> String {
    let server_extra = server_extra.unwrap_or_default();
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n{server_extra}    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/ws\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("http://{upstream_addr}"),
        server_extra = if server_extra.is_empty() {
            String::new()
        } else {
            format!("        {server_extra}\n")
        },
        ready_route = READY_ROUTE_CONFIG,
    )
}
