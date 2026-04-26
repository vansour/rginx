use super::*;

pub(super) fn fetch_http_text_response(
    listen_addr: SocketAddr,
    host: &str,
    path: &str,
) -> Result<(u16, String), String> {
    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;

    write!(stream, "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")
        .map_err(|error| format!("failed to write request: {error}"))?;
    stream.flush().map_err(|error| format!("failed to flush request: {error}"))?;
    read_text_response_from_stream(&mut stream)
}

pub(super) fn fetch_https_text_response(
    listen_addr: SocketAddr,
    host: &str,
    path: &str,
    server_name: &str,
) -> Result<(u16, String), String> {
    let tcp = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    tcp.set_read_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    tcp.set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;

    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(InsecureServerCertVerifier::new()))
        .with_no_client_auth();
    let server_name = ServerName::try_from(server_name.to_string())
        .map_err(|error| format!("invalid TLS server name `{server_name}`: {error}"))?;
    let connection = ClientConnection::new(Arc::new(config), server_name)
        .map_err(|error| format!("failed to build TLS client: {error}"))?;
    let mut stream = StreamOwned::new(connection, tcp);

    write!(stream, "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")
        .map_err(|error| format!("failed to write HTTPS request: {error}"))?;
    stream.flush().map_err(|error| format!("failed to flush HTTPS request: {error}"))?;
    read_text_response_from_stream(&mut stream)
}

fn read_text_response_from_stream(stream: &mut impl Read) -> Result<(u16, String), String> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    let head_end = loop {
        let read =
            stream.read(&mut chunk).map_err(|error| format!("failed to read response: {error}"))?;
        if read == 0 {
            return Err("stream closed before the HTTP response head was complete".to_string());
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };

    let head = String::from_utf8(buffer[..head_end].to_vec())
        .map_err(|error| format!("response head was not valid UTF-8: {error}"))?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| format!("missing status line: {head:?}"))?
        .parse::<u16>()
        .map_err(|error| format!("invalid status code: {error}"))?;

    let headers = head.lines().skip(1).collect::<Vec<_>>();
    let mut body_bytes = buffer[head_end..].to_vec();

    if let Some(content_length) = content_length(&headers)? {
        while body_bytes.len() < content_length {
            let read = stream
                .read(&mut chunk)
                .map_err(|error| format!("failed to read response body: {error}"))?;
            if read == 0 {
                return Err("stream closed before response body reached content-length".to_string());
            }
            body_bytes.extend_from_slice(&chunk[..read]);
        }
        body_bytes.truncate(content_length);
    } else if is_chunked(&headers) {
        body_bytes = decode_chunked_body(stream, body_bytes)?;
    } else {
        stream
            .read_to_end(&mut body_bytes)
            .map_err(|error| format!("failed to read response body: {error}"))?;
    }

    let body = String::from_utf8(body_bytes)
        .map_err(|error| format!("response body was not valid UTF-8: {error}"))?;
    Ok((status, body))
}

fn content_length(headers: &[&str]) -> Result<Option<usize>, String> {
    let mut value = None;
    for header in headers {
        let Some((name, raw_value)) = header.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            value = Some(
                raw_value
                    .trim()
                    .parse::<usize>()
                    .map_err(|error| format!("invalid content-length header: {error}"))?,
            );
        }
    }
    Ok(value)
}

fn is_chunked(headers: &[&str]) -> bool {
    headers.iter().any(|header| {
        header.split_once(':').is_some_and(|(name, value)| {
            name.trim().eq_ignore_ascii_case("transfer-encoding")
                && value.split(',').any(|item| item.trim().eq_ignore_ascii_case("chunked"))
        })
    })
}

fn decode_chunked_body(stream: &mut impl Read, mut buffer: Vec<u8>) -> Result<Vec<u8>, String> {
    let mut decoded = Vec::new();
    let mut scratch = [0u8; 1024];

    loop {
        let line_end = loop {
            if let Some(position) = buffer.windows(2).position(|window| window == b"\r\n") {
                break position;
            }
            let read = stream
                .read(&mut scratch)
                .map_err(|error| format!("failed to read chunk header: {error}"))?;
            if read == 0 {
                return Err("stream closed before chunk header was complete".to_string());
            }
            buffer.extend_from_slice(&scratch[..read]);
        };

        let line = String::from_utf8(buffer[..line_end].to_vec())
            .map_err(|error| format!("chunk header was not valid UTF-8: {error}"))?;
        let chunk_len = usize::from_str_radix(line.trim(), 16)
            .map_err(|error| format!("invalid chunk length `{line}`: {error}"))?;
        buffer.drain(..line_end + 2);

        if chunk_len == 0 {
            loop {
                if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                    buffer.drain(..position + 4);
                    return Ok(decoded);
                }
                let read = stream
                    .read(&mut scratch)
                    .map_err(|error| format!("failed to read chunk trailers: {error}"))?;
                if read == 0 {
                    return Ok(decoded);
                }
                buffer.extend_from_slice(&scratch[..read]);
            }
        }

        while buffer.len() < chunk_len + 2 {
            let read = stream
                .read(&mut scratch)
                .map_err(|error| format!("failed to read chunk body: {error}"))?;
            if read == 0 {
                return Err("stream closed before chunk body was complete".to_string());
            }
            buffer.extend_from_slice(&scratch[..read]);
        }

        decoded.extend_from_slice(&buffer[..chunk_len]);
        buffer.drain(..chunk_len + 2);
    }
}
