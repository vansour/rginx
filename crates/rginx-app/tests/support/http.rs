use super::*;

pub fn reserve_loopback_addr() -> SocketAddr {
    let listener =
        TcpListener::bind(("127.0.0.1", 0)).expect("ephemeral loopback listener should bind");
    let addr = listener.local_addr().expect("listener addr should be available");
    drop(listener);
    addr
}

pub fn read_http_head(stream: &mut TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 256];

    loop {
        let read = stream.read(&mut chunk).expect("HTTP head should be readable");
        assert!(read > 0, "stream closed before the HTTP head was complete");
        buffer.extend_from_slice(&chunk[..read]);

        if let Some(head_end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            return String::from_utf8(buffer[..head_end + 4].to_vec())
                .expect("HTTP head should be valid UTF-8");
        }
    }
}

pub fn connect_http_client(listen_addr: SocketAddr, read_timeout: Duration) -> TcpStream {
    let stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .expect("client should connect");
    stream.set_read_timeout(Some(read_timeout)).expect("client read timeout should set");
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .expect("client write timeout should set");
    stream
}

pub fn read_http_head_and_pending(stream: &mut TcpStream) -> (String, Vec<u8>) {
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

#[derive(Debug, PartialEq, Eq)]
pub enum HttpChunkRead {
    Chunk(Vec<u8>),
    End,
    TimedOut,
}

pub fn read_http_chunk(stream: &mut TcpStream, pending: &mut Vec<u8>) -> HttpChunkRead {
    let mut scratch = [0u8; 256];

    let line_end = loop {
        if let Some(position) = pending.windows(2).position(|window| window == b"\r\n") {
            break position;
        }
        match stream.read(&mut scratch) {
            Ok(0) => return HttpChunkRead::End,
            Ok(read) => pending.extend_from_slice(&scratch[..read]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return HttpChunkRead::TimedOut;
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
            Ok(0) => return HttpChunkRead::End,
            Ok(read) => pending.extend_from_slice(&scratch[..read]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return HttpChunkRead::TimedOut;
            }
            Err(error) => panic!("failed to read chunk payload: {error}"),
        }
    }

    if chunk_len == 0 {
        pending.drain(..2);
        return HttpChunkRead::End;
    }

    let chunk = pending[..chunk_len].to_vec();
    pending.drain(..chunk_len + 2);
    HttpChunkRead::Chunk(chunk)
}

pub fn spawn_scripted_chunked_response_server(
    expected_request_line: &'static str,
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
            request.starts_with(expected_request_line),
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
        }
        let _ = stream.write_all(b"0\r\n\r\n");
        let _ = stream.flush();
    });

    (listen_addr, task)
}

pub fn apply_tls_placeholders(config: String, cert_path: &Path, key_path: &Path) -> String {
    config
        .replace("__CERT_PATH__", &cert_path.display().to_string())
        .replace("__KEY_PATH__", &key_path.display().to_string())
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
