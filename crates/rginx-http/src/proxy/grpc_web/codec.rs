use super::*;

pub(crate) fn invalid_grpc_web_body(message: &str) -> BoxError {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message).into()
}

pub(crate) enum ParsedGrpcWebRequestFrame {
    Data(Bytes),
    Trailers(HeaderMap),
}

pub(crate) fn decode_grpc_web_request_frame(
    buffer: &mut BytesMut,
    inner_finished: bool,
    saw_grpc_web_trailers: bool,
) -> Result<Option<ParsedGrpcWebRequestFrame>, BoxError> {
    if buffer.is_empty() {
        return Ok(None);
    }

    if saw_grpc_web_trailers {
        return Err(invalid_grpc_web_body("grpc-web request trailer frame must be terminal"));
    }

    if buffer.len() < 5 {
        if inner_finished {
            return Err(invalid_grpc_web_body("incomplete grpc-web request frame header"));
        }
        return Ok(None);
    }

    let flags = buffer[0];
    let len = u32::from_be_bytes([buffer[1], buffer[2], buffer[3], buffer[4]]) as usize;
    let frame_len = 5usize.saturating_add(len);
    if buffer.len() < frame_len {
        if inner_finished {
            return Err(invalid_grpc_web_body("incomplete grpc-web request frame payload"));
        }
        return Ok(None);
    }

    let frame = buffer.split_to(frame_len).freeze();
    if (flags & 0x80) != 0 {
        Ok(Some(ParsedGrpcWebRequestFrame::Trailers(decode_grpc_web_trailer_block(&frame[5..])?)))
    } else {
        Ok(Some(ParsedGrpcWebRequestFrame::Data(frame)))
    }
}

fn decode_grpc_web_trailer_block(payload: &[u8]) -> Result<HeaderMap, BoxError> {
    let mut trailers = HeaderMap::new();

    for line in payload.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            continue;
        }

        let Some(separator) = line.iter().position(|byte| *byte == b':') else {
            return Err(invalid_grpc_web_body("grpc-web trailer line should contain ':'"));
        };
        let (name, value) = line.split_at(separator);
        let value = &value[1..];
        let name = HeaderName::from_bytes(name).map_err(|_| {
            invalid_grpc_web_body("grpc-web trailer name should be a valid HTTP header")
        })?;
        let value = std::str::from_utf8(value)
            .map_err(|_| invalid_grpc_web_body("grpc-web trailer value should be valid utf-8"))?
            .trim();
        let value = HeaderValue::from_str(value).map_err(|_| {
            invalid_grpc_web_body("grpc-web trailer value should be a valid HTTP header")
        })?;
        trailers.append(name, value);
    }

    Ok(trailers)
}

pub(crate) fn decode_grpc_web_text_chunk(
    carryover: &mut BytesMut,
    data: &[u8],
) -> Result<Option<Bytes>, BoxError> {
    for byte in data {
        if !byte.is_ascii_whitespace() {
            carryover.extend_from_slice(&[*byte]);
        }
    }

    let complete_len = carryover.len() / 4 * 4;
    if complete_len == 0 {
        return Ok(None);
    }

    let encoded = carryover.split_to(complete_len);
    let mut decoded = Vec::new();
    for quantum in encoded.chunks_exact(4) {
        let chunk = STANDARD.decode(quantum).map_err(|error| {
            invalid_grpc_web_body(&format!("invalid grpc-web-text base64 chunk: {error}"))
        })?;
        decoded.extend_from_slice(&chunk);
    }

    Ok((!decoded.is_empty()).then(|| Bytes::from(decoded)))
}

pub(crate) fn decode_grpc_web_text_final(
    carryover: &mut BytesMut,
) -> Result<Option<Bytes>, BoxError> {
    if carryover.is_empty() {
        return Ok(None);
    }

    if carryover.len() % 4 != 0 {
        return Err(invalid_grpc_web_body("incomplete grpc-web-text base64 body"));
    }

    let encoded = carryover.split();
    let mut decoded = Vec::new();
    for quantum in encoded.chunks_exact(4) {
        let chunk = STANDARD.decode(quantum).map_err(|error| {
            invalid_grpc_web_body(&format!("invalid grpc-web-text base64 chunk: {error}"))
        })?;
        decoded.extend_from_slice(&chunk);
    }

    Ok((!decoded.is_empty()).then(|| Bytes::from(decoded)))
}

pub(crate) fn encode_grpc_web_text_chunk(carryover: &mut BytesMut, data: &[u8]) -> Option<Bytes> {
    carryover.extend_from_slice(data);
    let complete_len = carryover.len() / 3 * 3;
    if complete_len == 0 {
        return None;
    }

    let chunk = carryover.split_to(complete_len);
    Some(Bytes::from(STANDARD_NO_PAD.encode(chunk.as_ref())))
}

pub(crate) fn flush_grpc_web_text_chunk(carryover: &mut BytesMut) -> Option<Bytes> {
    if carryover.is_empty() {
        return None;
    }

    let chunk = carryover.split();
    Some(Bytes::from(STANDARD.encode(chunk.as_ref())))
}

pub(crate) fn extract_grpc_initial_trailers(headers: &mut HeaderMap) -> Option<HeaderMap> {
    let mut trailers = HeaderMap::new();

    for name in ["grpc-status", "grpc-message", "grpc-status-details-bin"] {
        if let Some(value) = headers.remove(name) {
            trailers.insert(HeaderName::from_static(name), value);
        }
    }

    (!trailers.is_empty()).then_some(trailers)
}

pub(crate) fn encode_grpc_web_trailers(trailers: &HeaderMap) -> Bytes {
    let mut trailer_block = Vec::new();
    for (name, value) in trailers {
        trailer_block.extend_from_slice(name.as_str().as_bytes());
        trailer_block.extend_from_slice(b": ");
        trailer_block.extend_from_slice(value.as_bytes());
        trailer_block.extend_from_slice(b"\r\n");
    }

    let mut encoded = Vec::with_capacity(5 + trailer_block.len());
    encoded.push(0x80);
    encoded.extend_from_slice(&(trailer_block.len() as u32).to_be_bytes());
    encoded.extend_from_slice(&trailer_block);
    Bytes::from(encoded)
}
