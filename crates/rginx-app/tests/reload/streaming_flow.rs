use super::*;

#[test]
fn sighup_reload_drains_inflight_streaming_response_before_switching_routes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (upstream_addr, upstream_task) = spawn_scripted_chunked_response_server(
        b"before reload\n",
        Duration::from_millis(900),
        Some(b"after chunk\n"),
    );
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn_with_config(
        "rginx-reload-streaming-response",
        streaming_proxy_config(listen_addr, upstream_addr),
    );

    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let mut client = connect_http_stream(listen_addr);
    write!(client, "GET / HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n")
        .expect("streaming request should write");
    client.flush().expect("streaming request should flush");

    let (head, mut pending) = read_http_head_and_pending(&mut client);
    assert!(head.starts_with("HTTP/1.1 200"), "unexpected response head: {head:?}");
    assert!(
        head.to_ascii_lowercase().contains("\r\ntransfer-encoding: chunked\r\n"),
        "streaming reload response should remain chunked: {head:?}"
    );
    assert_eq!(
        read_http_chunk(&mut client, &mut pending)
            .expect("first chunk should arrive before reload"),
        b"before reload\n"
    );

    server.write_return_config(listen_addr, "after reload\n");
    server.send_signal(libc::SIGHUP);

    assert_eq!(
        read_http_chunk(&mut client, &mut pending)
            .expect("second chunk should arrive after reload"),
        b"after chunk\n"
    );
    assert!(read_http_chunk(&mut client, &mut pending).is_none(), "stream should end cleanly");

    server.wait_for_body(listen_addr, "after reload\n", Duration::from_secs(5));
    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.join().expect("upstream thread should complete");
}

#[test]
fn nginx_style_restart_command_drains_inflight_streaming_response_before_old_process_exits() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (upstream_addr, upstream_task) = spawn_scripted_chunked_response_server(
        b"before restart\n",
        Duration::from_millis(900),
        Some(b"after chunk\n"),
    );
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn_with_config(
        "rginx-restart-streaming-response",
        streaming_proxy_config(listen_addr, upstream_addr),
    );

    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));
    let old_pid = read_pid_file(&server.pid_path());

    let mut client = connect_http_stream(listen_addr);
    write!(client, "GET / HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n")
        .expect("streaming request should write");
    client.flush().expect("streaming request should flush");

    let (head, mut pending) = read_http_head_and_pending(&mut client);
    assert!(head.starts_with("HTTP/1.1 200"), "unexpected response head: {head:?}");
    assert_eq!(
        read_http_chunk(&mut client, &mut pending)
            .expect("first chunk should arrive before restart"),
        b"before restart\n"
    );

    server.write_return_config(listen_addr, "after restart\n");
    let output = server.send_cli_signal("restart");
    assert!(output.status.success(), "rginx -s restart should succeed: {}", render_output(&output));

    let new_pid = wait_for_pid_change(&server.pid_path(), old_pid, Duration::from_secs(10));
    assert_eq!(
        read_http_chunk(&mut client, &mut pending)
            .expect("second chunk should arrive while old process drains"),
        b"after chunk\n"
    );
    assert!(read_http_chunk(&mut client, &mut pending).is_none(), "stream should end cleanly");

    let status = server.wait_for_exit(Duration::from_secs(10));
    assert!(status.success(), "old process should exit cleanly after draining: {status}");
    wait_for_body(listen_addr, "after restart\n", Duration::from_secs(10));

    let quit = server.send_cli_signal("quit");
    assert!(
        quit.status.success(),
        "rginx -s quit should stop replacement process: {}",
        render_output(&quit)
    );
    wait_for_process_exit(new_pid, Duration::from_secs(10));
    upstream_task.join().expect("upstream thread should complete");
}

fn connect_http_stream(listen_addr: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .expect("client should connect");
    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    stream.set_write_timeout(Some(Duration::from_millis(500))).unwrap();
    stream
}

fn read_http_head_and_pending(stream: &mut TcpStream) -> (String, Vec<u8>) {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 256];

    loop {
        let read = stream.read(&mut chunk).expect("HTTP head should be readable");
        assert!(read > 0, "stream closed before the HTTP head was complete");
        buffer.extend_from_slice(&chunk[..read]);

        if let Some(head_end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            return (
                String::from_utf8(buffer[..head_end + 4].to_vec())
                    .expect("HTTP head should be valid UTF-8"),
                buffer[head_end + 4..].to_vec(),
            );
        }
    }
}

fn read_http_chunk(stream: &mut TcpStream, pending: &mut Vec<u8>) -> Option<Vec<u8>> {
    let mut scratch = [0u8; 256];

    let line_end = loop {
        if let Some(position) = pending.windows(2).position(|window| window == b"\r\n") {
            break position;
        }
        match stream.read(&mut scratch) {
            Ok(0) => return None,
            Ok(read) => pending.extend_from_slice(&scratch[..read]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return None;
            }
            Err(error) => panic!("failed to read chunk header: {error}"),
        }
    };

    let line =
        String::from_utf8(pending[..line_end].to_vec()).expect("chunk header should be utf-8");
    let chunk_len =
        usize::from_str_radix(line.trim(), 16).expect("chunk length should be valid hex");
    pending.drain(..line_end + 2);

    while pending.len() < chunk_len + 2 {
        match stream.read(&mut scratch) {
            Ok(0) => return None,
            Ok(read) => pending.extend_from_slice(&scratch[..read]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return None;
            }
            Err(error) => panic!("failed to read chunk payload: {error}"),
        }
    }

    if chunk_len == 0 {
        pending.drain(..2);
        return None;
    }

    let chunk = pending[..chunk_len].to_vec();
    pending.drain(..chunk_len + 2);
    Some(chunk)
}

fn spawn_scripted_chunked_response_server(
    first_chunk: &'static [u8],
    pause_after_first_chunk: Duration,
    second_chunk: Option<&'static [u8]>,
) -> (SocketAddr, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .expect("streaming upstream listener should bind");
    let listen_addr = listener.local_addr().expect("upstream addr should be available");

    let task = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("upstream should accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("upstream read timeout should be configurable");
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .expect("upstream write timeout should be configurable");

        let request = super::support::read_http_head(&mut stream);
        assert!(
            request.starts_with("GET / HTTP/1.1\r\n"),
            "unexpected upstream request: {request:?}"
        );

        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .expect("upstream response head should write");
        write_chunk(&mut stream, first_chunk);
        std::thread::sleep(pause_after_first_chunk);
        if let Some(second_chunk) = second_chunk {
            write_chunk(&mut stream, second_chunk);
        }
        stream.write_all(b"0\r\n\r\n").expect("terminal chunk should write");
        stream.flush().expect("terminal chunk should flush");
    });

    (listen_addr, task)
}

fn write_chunk(stream: &mut TcpStream, chunk: &[u8]) {
    write!(stream, "{:x}\r\n", chunk.len()).expect("chunk header should write");
    stream.write_all(chunk).expect("chunk payload should write");
    stream.write_all(b"\r\n").expect("chunk payload should terminate with CRLF");
    stream.flush().expect("chunk should flush");
}

fn streaming_proxy_config(listen_addr: SocketAddr, upstream_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            request_timeout_secs: Some(5),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n            response_buffering: Some(Off),\n            compression: Some(Off),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("http://{upstream_addr}"),
        ready_route = READY_ROUTE_CONFIG,
    )
}
