use super::*;

#[derive(Debug, Clone)]
pub(super) struct GrpcWebMode {
    pub downstream_content_type: HeaderValue,
    pub upstream_content_type: HeaderValue,
    pub encoding: GrpcWebEncoding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GrpcWebEncoding {
    Binary,
    Text,
}

impl GrpcWebMode {
    pub(super) fn is_text(&self) -> bool {
        self.encoding == GrpcWebEncoding::Text
    }
}

pin_project! {
    pub(super) struct GrpcWebResponseBody<B> {
        #[pin]
        inner: B,
        pending_frame: Option<Bytes>,
        fallback_trailers: Option<HeaderMap>,
        inner_finished: bool,
    }
}

impl<B> GrpcWebResponseBody<B> {
    pub(super) fn new(inner: B, fallback_trailers: Option<HeaderMap>) -> Self {
        Self { inner, pending_frame: None, fallback_trailers, inner_finished: false }
    }
}

impl<B> hyper::body::Body for GrpcWebResponseBody<B>
where
    B: hyper::body::Body<Data = Bytes>,
    B::Error: Into<BoxError>,
{
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();

        loop {
            if let Some(frame) = this.pending_frame.take() {
                return std::task::Poll::Ready(Some(Ok(Frame::data(frame))));
            }

            if *this.inner_finished {
                return std::task::Poll::Ready(None);
            }

            match this.inner.as_mut().poll_frame(cx) {
                std::task::Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
                    Ok(data) => return std::task::Poll::Ready(Some(Ok(Frame::data(data)))),
                    Err(frame) => match frame.into_trailers() {
                        Ok(trailers) => {
                            this.fallback_trailers.take();
                            *this.pending_frame = Some(encode_grpc_web_trailers(&trailers));
                        }
                        Err(_) => {
                            return std::task::Poll::Ready(Some(Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "unexpected upstream response frame for grpc-web translation",
                            )
                            .into())));
                        }
                    },
                },
                std::task::Poll::Ready(Some(Err(error))) => {
                    *this.inner_finished = true;
                    return std::task::Poll::Ready(Some(Err(error.into())));
                }
                std::task::Poll::Ready(None) => {
                    *this.inner_finished = true;
                    if let Some(trailers) = this.fallback_trailers.take()
                        && !trailers.is_empty()
                    {
                        *this.pending_frame = Some(encode_grpc_web_trailers(&trailers));
                        continue;
                    }
                    return std::task::Poll::Ready(None);
                }
                std::task::Poll::Pending => return std::task::Poll::Pending,
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner_finished && self.pending_frame.is_none()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

pin_project! {
    pub(super) struct GrpcWebTextDecodeBody<B> {
        #[pin]
        inner: B,
        pending_data: Option<Bytes>,
        pending_trailers: Option<HeaderMap>,
        carryover: BytesMut,
        inner_finished: bool,
    }
}

impl<B> GrpcWebTextDecodeBody<B> {
    pub(super) fn new(inner: B) -> Self {
        Self {
            inner,
            pending_data: None,
            pending_trailers: None,
            carryover: BytesMut::new(),
            inner_finished: false,
        }
    }
}

impl<B> hyper::body::Body for GrpcWebTextDecodeBody<B>
where
    B: hyper::body::Body<Data = Bytes>,
    B::Error: Into<BoxError>,
{
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();

        loop {
            if let Some(data) = this.pending_data.take() {
                return std::task::Poll::Ready(Some(Ok(Frame::data(data))));
            }

            if let Some(trailers) = this.pending_trailers.take() {
                return std::task::Poll::Ready(Some(Ok(Frame::trailers(trailers))));
            }

            if *this.inner_finished {
                return match decode_grpc_web_text_final(this.carryover) {
                    Ok(Some(data)) => std::task::Poll::Ready(Some(Ok(Frame::data(data)))),
                    Ok(None) => std::task::Poll::Ready(None),
                    Err(error) => std::task::Poll::Ready(Some(Err(error))),
                };
            }

            match this.inner.as_mut().poll_frame(cx) {
                std::task::Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
                    Ok(data) => match decode_grpc_web_text_chunk(this.carryover, &data) {
                        Ok(Some(decoded)) => {
                            return std::task::Poll::Ready(Some(Ok(Frame::data(decoded))));
                        }
                        Ok(None) => continue,
                        Err(error) => return std::task::Poll::Ready(Some(Err(error))),
                    },
                    Err(frame) => match frame.into_trailers() {
                        Ok(trailers) => match decode_grpc_web_text_final(this.carryover) {
                            Ok(Some(decoded)) => {
                                *this.pending_trailers = Some(trailers);
                                return std::task::Poll::Ready(Some(Ok(Frame::data(decoded))));
                            }
                            Ok(None) => {
                                return std::task::Poll::Ready(Some(Ok(Frame::trailers(trailers))));
                            }
                            Err(error) => return std::task::Poll::Ready(Some(Err(error))),
                        },
                        Err(_) => {
                            return std::task::Poll::Ready(Some(Err(invalid_grpc_web_body(
                                "unexpected downstream request frame for grpc-web-text decoding",
                            ))));
                        }
                    },
                },
                std::task::Poll::Ready(Some(Err(error))) => {
                    *this.inner_finished = true;
                    return std::task::Poll::Ready(Some(Err(error.into())));
                }
                std::task::Poll::Ready(None) => {
                    *this.inner_finished = true;
                    continue;
                }
                std::task::Poll::Pending => return std::task::Poll::Pending,
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner_finished
            && self.pending_data.is_none()
            && self.pending_trailers.is_none()
            && self.carryover.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

pin_project! {
    pub(super) struct GrpcWebTextEncodeBody<B> {
        #[pin]
        inner: B,
        pending_data: Option<Bytes>,
        carryover: BytesMut,
        inner_finished: bool,
    }
}

impl<B> GrpcWebTextEncodeBody<B> {
    pub(super) fn new(inner: B) -> Self {
        Self { inner, pending_data: None, carryover: BytesMut::new(), inner_finished: false }
    }
}

impl<B> hyper::body::Body for GrpcWebTextEncodeBody<B>
where
    B: hyper::body::Body<Data = Bytes>,
    B::Error: Into<BoxError>,
{
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();

        loop {
            if let Some(data) = this.pending_data.take() {
                return std::task::Poll::Ready(Some(Ok(Frame::data(data))));
            }

            if *this.inner_finished {
                if let Some(data) = flush_grpc_web_text_chunk(this.carryover) {
                    return std::task::Poll::Ready(Some(Ok(Frame::data(data))));
                }
                return std::task::Poll::Ready(None);
            }

            match this.inner.as_mut().poll_frame(cx) {
                std::task::Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
                    Ok(data) => {
                        if let Some(encoded) = encode_grpc_web_text_chunk(this.carryover, &data) {
                            return std::task::Poll::Ready(Some(Ok(Frame::data(encoded))));
                        }
                    }
                    Err(frame) => match frame.into_trailers() {
                        Ok(trailers) => {
                            let encoded_trailers = encode_grpc_web_trailers(&trailers);
                            if let Some(encoded) =
                                encode_grpc_web_text_chunk(this.carryover, &encoded_trailers)
                            {
                                return std::task::Poll::Ready(Some(Ok(Frame::data(encoded))));
                            }
                        }
                        Err(_) => {
                            return std::task::Poll::Ready(Some(Err(invalid_grpc_web_body(
                                "unexpected upstream response frame for grpc-web-text encoding",
                            ))));
                        }
                    },
                },
                std::task::Poll::Ready(Some(Err(error))) => {
                    *this.inner_finished = true;
                    return std::task::Poll::Ready(Some(Err(error.into())));
                }
                std::task::Poll::Ready(None) => {
                    *this.inner_finished = true;
                    continue;
                }
                std::task::Poll::Pending => return std::task::Poll::Pending,
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner_finished && self.pending_data.is_none() && self.carryover.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

pin_project! {
    pub(super) struct GrpcWebRequestBody<B> {
        #[pin]
        inner: B,
        inner_finished: bool,
        inner_trailers: Option<HeaderMap>,
        pending_data: Option<Bytes>,
        pending_trailers: Option<HeaderMap>,
        buffer: BytesMut,
        saw_grpc_web_trailers: bool,
    }
}

impl<B> GrpcWebRequestBody<B> {
    pub(super) fn new(inner: B) -> Self {
        Self {
            inner,
            inner_finished: false,
            inner_trailers: None,
            pending_data: None,
            pending_trailers: None,
            buffer: BytesMut::new(),
            saw_grpc_web_trailers: false,
        }
    }
}

impl<B> hyper::body::Body for GrpcWebRequestBody<B>
where
    B: hyper::body::Body<Data = Bytes>,
    B::Error: Into<BoxError>,
{
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();

        loop {
            if let Some(data) = this.pending_data.take() {
                return std::task::Poll::Ready(Some(Ok(Frame::data(data))));
            }

            if let Some(trailers) = this.pending_trailers.take() {
                return std::task::Poll::Ready(Some(Ok(Frame::trailers(trailers))));
            }

            match decode_grpc_web_request_frame(
                this.buffer,
                *this.inner_finished,
                *this.saw_grpc_web_trailers,
            ) {
                Ok(Some(ParsedGrpcWebRequestFrame::Data(data))) => {
                    return std::task::Poll::Ready(Some(Ok(Frame::data(data))));
                }
                Ok(Some(ParsedGrpcWebRequestFrame::Trailers(mut trailers))) => {
                    *this.saw_grpc_web_trailers = true;
                    if let Some(inner_trailers) = this.inner_trailers.take() {
                        append_header_map(&mut trailers, &inner_trailers);
                    }
                    if trailers.is_empty() {
                        continue;
                    }
                    return std::task::Poll::Ready(Some(Ok(Frame::trailers(trailers))));
                }
                Ok(None) => {}
                Err(error) => return std::task::Poll::Ready(Some(Err(error))),
            }

            if *this.inner_finished {
                if let Some(trailers) = this.inner_trailers.take() {
                    return std::task::Poll::Ready(Some(Ok(Frame::trailers(trailers))));
                }
                return std::task::Poll::Ready(None);
            }

            match this.inner.as_mut().poll_frame(cx) {
                std::task::Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
                    Ok(data) => {
                        if *this.saw_grpc_web_trailers && !data.is_empty() {
                            return std::task::Poll::Ready(Some(Err(invalid_grpc_web_body(
                                "grpc-web request trailer frame must be terminal",
                            ))));
                        }
                        this.buffer.extend_from_slice(&data);
                    }
                    Err(frame) => match frame.into_trailers() {
                        Ok(trailers) => {
                            if let Some(existing) = this.inner_trailers.as_mut() {
                                append_header_map(existing, &trailers);
                            } else {
                                *this.inner_trailers = Some(trailers);
                            }
                            *this.inner_finished = true;
                        }
                        Err(_) => {
                            return std::task::Poll::Ready(Some(Err(invalid_grpc_web_body(
                                "unexpected downstream request frame for grpc-web decoding",
                            ))));
                        }
                    },
                },
                std::task::Poll::Ready(Some(Err(error))) => {
                    *this.inner_finished = true;
                    return std::task::Poll::Ready(Some(Err(error.into())));
                }
                std::task::Poll::Ready(None) => {
                    *this.inner_finished = true;
                }
                std::task::Poll::Pending => return std::task::Poll::Pending,
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner_finished
            && self.inner_trailers.is_none()
            && self.pending_data.is_none()
            && self.pending_trailers.is_none()
            && self.buffer.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

fn invalid_grpc_web_body(message: &str) -> BoxError {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message).into()
}

enum ParsedGrpcWebRequestFrame {
    Data(Bytes),
    Trailers(HeaderMap),
}

fn decode_grpc_web_request_frame(
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

pub(super) fn decode_grpc_web_text_chunk(
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

pub(super) fn decode_grpc_web_text_final(
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

pub(super) fn encode_grpc_web_text_chunk(carryover: &mut BytesMut, data: &[u8]) -> Option<Bytes> {
    carryover.extend_from_slice(data);
    let complete_len = carryover.len() / 3 * 3;
    if complete_len == 0 {
        return None;
    }

    let chunk = carryover.split_to(complete_len);
    Some(Bytes::from(STANDARD_NO_PAD.encode(chunk.as_ref())))
}

pub(super) fn flush_grpc_web_text_chunk(carryover: &mut BytesMut) -> Option<Bytes> {
    if carryover.is_empty() {
        return None;
    }

    let chunk = carryover.split();
    Some(Bytes::from(STANDARD.encode(chunk.as_ref())))
}

pub(super) fn extract_grpc_initial_trailers(headers: &mut HeaderMap) -> Option<HeaderMap> {
    let mut trailers = HeaderMap::new();

    for name in ["grpc-status", "grpc-message", "grpc-status-details-bin"] {
        if let Some(value) = headers.remove(name) {
            trailers.insert(HeaderName::from_static(name), value);
        }
    }

    (!trailers.is_empty()).then_some(trailers)
}

pub(super) fn encode_grpc_web_trailers(trailers: &HeaderMap) -> Bytes {
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
