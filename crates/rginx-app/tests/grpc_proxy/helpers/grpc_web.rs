use super::*;

pub(crate) fn decode_grpc_web_response(bytes: &[u8]) -> (Vec<Bytes>, HeaderMap) {
    let mut offset = 0usize;
    let mut frames = Vec::new();
    let mut trailers = HeaderMap::new();

    while offset < bytes.len() {
        assert!(
            bytes.len().saturating_sub(offset) >= 5,
            "grpc-web frame should include a 5-byte header"
        );
        let flags = bytes[offset];
        let len = u32::from_be_bytes([
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
            bytes[offset + 4],
        ]) as usize;
        offset += 5;
        assert!(
            bytes.len().saturating_sub(offset) >= len,
            "grpc-web frame payload should be fully present"
        );
        let payload = &bytes[offset..offset + len];
        offset += len;

        if (flags & 0x80) != 0 {
            for line in payload.split(|byte| *byte == b'\n') {
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                if line.is_empty() {
                    continue;
                }

                let Some(separator) = line.iter().position(|byte| *byte == b':') else {
                    panic!("grpc-web trailer line should contain ':'");
                };
                let (name, value) = line.split_at(separator);
                let value = &value[1..];
                let name =
                    std::str::from_utf8(name).expect("grpc-web trailer name should be utf-8");
                let value = std::str::from_utf8(value)
                    .expect("grpc-web trailer value should be utf-8")
                    .trim();
                trailers.insert(
                    name.parse::<HeaderName>().expect("grpc-web trailer name should be valid"),
                    HeaderValue::from_str(value).expect("grpc-web trailer value should be valid"),
                );
            }
        } else {
            frames.push(Bytes::copy_from_slice(payload));
        }
    }

    (frames, trailers)
}

pub(crate) fn encode_grpc_web_text_payload(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

pub(crate) fn grpc_web_request_with_trailers() -> Bytes {
    let mut request = BytesMut::from(GRPC_REQUEST_FRAME);
    request.extend_from_slice(grpc_web_trailer_frame().as_ref());
    request.freeze()
}

pub(crate) fn grpc_web_trailer_frame() -> Bytes {
    let block = b"x-client-trailer: sent\r\nx-request-checksum: abc123\r\n";
    let mut frame = Vec::with_capacity(5 + block.len());
    frame.push(0x80);
    frame.extend_from_slice(&(block.len() as u32).to_be_bytes());
    frame.extend_from_slice(block);
    Bytes::from(frame)
}

pub(crate) fn grpc_health_response_frame(serving_status: u8) -> Bytes {
    Bytes::from(vec![0x00, 0x00, 0x00, 0x00, 0x02, 0x08, serving_status])
}

pub(crate) fn decode_grpc_web_text_payload(bytes: &[u8]) -> Vec<u8> {
    let filtered =
        bytes.iter().copied().filter(|byte| !byte.is_ascii_whitespace()).collect::<Vec<_>>();
    let mut decoded = Vec::new();

    for quantum in filtered.chunks_exact(4) {
        let chunk = STANDARD.decode(quantum).expect("grpc-web-text payload should be valid base64");
        decoded.extend_from_slice(&chunk);
    }

    assert_eq!(
        filtered.len() % 4,
        0,
        "grpc-web-text payload should end on a base64 quantum boundary"
    );
    decoded
}
