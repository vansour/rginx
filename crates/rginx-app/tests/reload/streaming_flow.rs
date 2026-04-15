use super::*;

#[test]
fn sighup_reload_drains_inflight_streaming_response_before_switching_routes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (upstream_addr, upstream_task) = super::support::spawn_scripted_chunked_response_server(
        "GET / HTTP/1.1\r\n",
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

    let mut client = super::support::connect_http_client(listen_addr, Duration::from_secs(3));
    write!(client, "GET / HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n")
        .expect("streaming request should write");
    client.flush().expect("streaming request should flush");

    let (head, mut pending) = super::support::read_http_head_and_pending(&mut client);
    assert!(head.starts_with("HTTP/1.1 200"), "unexpected response head: {head:?}");
    assert!(
        head.to_ascii_lowercase().contains("\r\ntransfer-encoding: chunked\r\n"),
        "streaming reload response should remain chunked: {head:?}"
    );
    assert_eq!(
        super::support::read_http_chunk(&mut client, &mut pending),
        super::support::HttpChunkRead::Chunk(b"before reload\n".to_vec())
    );

    server.write_return_config(listen_addr, "after reload\n");
    server.send_signal(libc::SIGHUP);

    assert_eq!(
        super::support::read_http_chunk(&mut client, &mut pending),
        super::support::HttpChunkRead::Chunk(b"after chunk\n".to_vec())
    );
    assert!(
        super::support::read_http_chunk(&mut client, &mut pending)
            == super::support::HttpChunkRead::End,
        "stream should end cleanly"
    );

    server.wait_for_body(listen_addr, "after reload\n", Duration::from_secs(5));
    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.join().expect("upstream thread should complete");
}

#[test]
fn nginx_style_restart_command_drains_inflight_streaming_response_before_old_process_exits() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (upstream_addr, upstream_task) = super::support::spawn_scripted_chunked_response_server(
        "GET / HTTP/1.1\r\n",
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

    let mut client = super::support::connect_http_client(listen_addr, Duration::from_secs(3));
    write!(client, "GET / HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n")
        .expect("streaming request should write");
    client.flush().expect("streaming request should flush");

    let (head, mut pending) = super::support::read_http_head_and_pending(&mut client);
    assert!(head.starts_with("HTTP/1.1 200"), "unexpected response head: {head:?}");
    assert_eq!(
        super::support::read_http_chunk(&mut client, &mut pending),
        super::support::HttpChunkRead::Chunk(b"before restart\n".to_vec())
    );

    server.write_return_config(listen_addr, "after restart\n");
    let output = server.send_cli_signal("restart");
    assert!(output.status.success(), "rginx -s restart should succeed: {}", render_output(&output));

    let new_pid = wait_for_pid_change(&server.pid_path(), old_pid, Duration::from_secs(10));
    assert_eq!(
        super::support::read_http_chunk(&mut client, &mut pending),
        super::support::HttpChunkRead::Chunk(b"after chunk\n".to_vec())
    );
    assert!(
        super::support::read_http_chunk(&mut client, &mut pending)
            == super::support::HttpChunkRead::End,
        "stream should end cleanly"
    );

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

fn streaming_proxy_config(listen_addr: SocketAddr, upstream_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            request_timeout_secs: Some(5),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n            response_buffering: Some(Off),\n            compression: Some(Off),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("http://{upstream_addr}"),
        ready_route = READY_ROUTE_CONFIG,
    )
}
