use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, read_http_head, reserve_loopback_addr};

#[test]
fn proxy_streaming_download_delivers_first_chunk_before_upstream_completes() {
    let (upstream_addr, upstream_task) = spawn_scripted_chunked_response_server(
        b"first\n",
        Duration::from_millis(900),
        Some(b"second\n"),
    );
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-streaming-download", |_| {
        proxy_streaming_config(listen_addr, upstream_addr, "")
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let mut client = connect_http_client(listen_addr);
    write!(client, "GET /api/stream HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n")
        .expect("streaming request should write");
    client.flush().expect("streaming request should flush");

    let started = Instant::now();
    let (head, mut pending) = read_http_head_and_pending(&mut client);
    let head_lower = head.to_ascii_lowercase();
    assert!(head.starts_with("HTTP/1.1 200"), "unexpected response head: {head:?}");
    assert!(
        head_lower.contains("\r\ntransfer-encoding: chunked\r\n"),
        "streaming response should remain chunked, got {head:?}"
    );

    let first =
        read_http_chunk(&mut client, &mut pending).expect("first streaming chunk should arrive");
    assert_eq!(first, b"first\n");
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "first chunk should arrive before the upstream finishes streaming, elapsed={:?}",
        started.elapsed()
    );

    let second =
        read_http_chunk(&mut client, &mut pending).expect("second streaming chunk should arrive");
    assert_eq!(second, b"second\n");
    assert_eq!(
        read_http_chunk(&mut client, &mut pending),
        None,
        "stream should end after the scripted chunks"
    );

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.join().expect("upstream thread should complete");
}

#[test]
fn route_streaming_response_idle_timeout_closes_stalled_proxy_downloads() {
    let (upstream_addr, upstream_task) =
        spawn_scripted_chunked_response_server(b"hello\n", Duration::from_secs(2), Some(b"late\n"));
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-streaming-download-idle-timeout", |_| {
        proxy_streaming_config(
            listen_addr,
            upstream_addr,
            "            streaming_response_idle_timeout_secs: Some(1),\n",
        )
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let mut client = connect_http_client(listen_addr);
    write!(client, "GET /api/stream HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n")
        .expect("streaming request should write");
    client.flush().expect("streaming request should flush");

    let (head, mut pending) = read_http_head_and_pending(&mut client);
    assert!(head.starts_with("HTTP/1.1 200"), "unexpected response head: {head:?}");
    let first = read_http_chunk(&mut client, &mut pending)
        .expect("first chunk should arrive before the stall");
    assert_eq!(first, b"hello\n");

    let stalled_at = Instant::now();
    let follow_up = read_http_chunk(&mut client, &mut pending);
    assert!(
        follow_up.is_none(),
        "stalled streaming response should terminate instead of delivering another chunk: {follow_up:?}"
    );
    assert!(
        stalled_at.elapsed() < Duration::from_millis(1800),
        "route idle timeout should cut off the stalled response before the upstream resumes, elapsed={:?}",
        stalled_at.elapsed()
    );

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.join().expect("upstream thread should complete");
}

fn connect_http_client(listen_addr: SocketAddr) -> TcpStream {
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
) -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("upstream listener should bind");
    let listen_addr = listener.local_addr().expect("upstream listener addr should be available");

    let task = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("upstream should accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("upstream read timeout should be configurable");
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .expect("upstream write timeout should be configurable");

        let request = read_http_head(&mut stream);
        assert!(
            request.starts_with("GET /stream HTTP/1.1\r\n"),
            "unexpected upstream request line: {request:?}"
        );

        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .expect("upstream response head should write");
        write_chunk(&mut stream, first_chunk);
        thread::sleep(pause_after_first_chunk);
        if let Some(second_chunk) = second_chunk {
            let _ = write_chunk_result(&mut stream, second_chunk);
            let _ = stream.write_all(b"0\r\n\r\n");
            let _ = stream.flush();
        }
    });

    (listen_addr, task)
}

fn write_chunk(stream: &mut TcpStream, chunk: &[u8]) {
    write_chunk_result(stream, chunk).expect("upstream chunk should write");
}

fn write_chunk_result(stream: &mut TcpStream, chunk: &[u8]) -> std::io::Result<()> {
    write!(stream, "{:x}\r\n", chunk.len())?;
    stream.write_all(chunk)?;
    stream.write_all(b"\r\n")?;
    stream.flush()
}

fn proxy_streaming_config(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    route_extra: &str,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            request_timeout_secs: Some(5),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/api\"),\n            handler: Proxy(\n                upstream: \"backend\",\n                strip_prefix: Some(\"/api\"),\n            ),\n            response_buffering: Some(Off),\n            compression: Some(Off),\n{route_extra}        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("http://{upstream_addr}"),
        ready_route = READY_ROUTE_CONFIG,
        route_extra = route_extra,
    )
}
