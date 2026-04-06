use super::access_log::{OwnedAccessLogContext, log_access_event};
use super::response::full_body;
use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GrpcObservability {
    pub(crate) protocol: String,
    pub(crate) service: String,
    pub(crate) method: String,
    pub(crate) status: Option<String>,
    pub(crate) message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GrpcRequestMetadata<'a> {
    pub(crate) protocol: &'static str,
    pub(crate) service: &'a str,
    pub(crate) method: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GrpcStatusCode {
    Cancelled,
    InvalidArgument,
    DeadlineExceeded,
    PermissionDenied,
    ResourceExhausted,
    Unimplemented,
    Unavailable,
}

impl GrpcStatusCode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Cancelled => "1",
            Self::InvalidArgument => "3",
            Self::DeadlineExceeded => "4",
            Self::PermissionDenied => "7",
            Self::ResourceExhausted => "8",
            Self::Unimplemented => "12",
            Self::Unavailable => "14",
        }
    }
}

#[derive(Debug, Clone)]
enum GrpcResponseFormat {
    Grpc { content_type: HeaderValue },
    GrpcWeb { content_type: HeaderValue, is_text: bool },
}

pub(super) fn grpc_observability(
    metadata: Option<GrpcRequestMetadata<'_>>,
    response_headers: &HeaderMap,
) -> Option<GrpcObservability> {
    let metadata = metadata?;
    let mut grpc = GrpcObservability {
        protocol: metadata.protocol.to_string(),
        service: metadata.service.to_string(),
        method: metadata.method.to_string(),
        status: None,
        message: None,
    };
    grpc.update_from_headers(response_headers);
    Some(grpc)
}

pub(super) fn grpc_request_metadata<'a>(
    headers: &HeaderMap,
    request_path: &'a str,
) -> Option<GrpcRequestMetadata<'a>> {
    let protocol = grpc_protocol(headers)?;
    let (service, method) = grpc_service_method(request_path)?;

    Some(GrpcRequestMetadata { protocol, service, method })
}

fn grpc_protocol(headers: &HeaderMap) -> Option<&'static str> {
    let (mime, _) =
        split_header_content_type(super::dispatch::header_value(headers, CONTENT_TYPE)?);
    let mime = mime.to_ascii_lowercase();
    if mime.starts_with("application/grpc-web-text") {
        Some("grpc-web-text")
    } else if mime.starts_with("application/grpc-web") {
        Some("grpc-web")
    } else if mime.starts_with("application/grpc") {
        Some("grpc")
    } else {
        None
    }
}

fn grpc_response_format(headers: &HeaderMap) -> Option<GrpcResponseFormat> {
    let content_type = headers.get(CONTENT_TYPE)?.clone();
    match grpc_protocol(headers)? {
        "grpc" => Some(GrpcResponseFormat::Grpc { content_type }),
        "grpc-web" => Some(GrpcResponseFormat::GrpcWeb { content_type, is_text: false }),
        "grpc-web-text" => Some(GrpcResponseFormat::GrpcWeb { content_type, is_text: true }),
        _ => None,
    }
}

pub(crate) fn grpc_error_response(
    request_headers: &HeaderMap,
    grpc_status: GrpcStatusCode,
    message: &str,
) -> Option<HttpResponse> {
    let format = grpc_response_format(request_headers)?;
    Some(build_grpc_error_response(format, grpc_status, message))
}

fn build_grpc_error_response(
    format: GrpcResponseFormat,
    grpc_status: GrpcStatusCode,
    message: &str,
) -> HttpResponse {
    let message = sanitize_grpc_message(message);
    let grpc_status_value = HeaderValue::from_static(grpc_status.as_str());
    let grpc_message_value = (!message.is_empty())
        .then(|| HeaderValue::from_str(&message).expect("sanitized gRPC message should be valid"));

    match format {
        GrpcResponseFormat::Grpc { content_type } => {
            let mut builder = Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, content_type)
                .header(CONTENT_LENGTH, "0")
                .header("grpc-status", grpc_status_value);
            if let Some(message) = grpc_message_value {
                builder = builder.header("grpc-message", message);
            }
            builder
                .body(full_body(Bytes::new()))
                .expect("gRPC error response builder should not fail")
        }
        GrpcResponseFormat::GrpcWeb { content_type, is_text } => {
            let mut trailers = HeaderMap::new();
            trailers.insert("grpc-status", grpc_status_value.clone());
            if let Some(message) = grpc_message_value.clone() {
                trailers.insert("grpc-message", message);
            }

            let body = encode_grpc_web_error_body(&trailers, is_text);
            let mut builder = Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, content_type)
                .header(CONTENT_LENGTH, body.len().to_string())
                .header("grpc-status", grpc_status_value);
            if let Some(message) = grpc_message_value {
                builder = builder.header("grpc-message", message);
            }
            builder.body(full_body(body)).expect("grpc-web error response builder should not fail")
        }
    }
}

fn sanitize_grpc_message(message: &str) -> String {
    message
        .trim()
        .chars()
        .map(|ch| if ch.is_ascii_control() { ' ' } else { ch })
        .collect::<String>()
}

fn encode_grpc_web_error_body(trailers: &HeaderMap, is_text: bool) -> Bytes {
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

    if is_text { Bytes::from(STANDARD.encode(&encoded)) } else { Bytes::from(encoded) }
}

impl GrpcObservability {
    pub(super) fn update_from_headers(&mut self, headers: &HeaderMap) {
        if let Some(status) =
            super::dispatch::header_value(headers, HeaderName::from_static("grpc-status"))
        {
            self.status = Some(status.to_string());
        }
        if let Some(message) =
            super::dispatch::header_value(headers, HeaderName::from_static("grpc-message"))
        {
            self.message = Some(message.to_string());
        }
    }
}

struct GrpcResponseFinalizer {
    format: Option<AccessLogFormat>,
    context: OwnedAccessLogContext,
    finalized: bool,
}

impl GrpcResponseFinalizer {
    fn new(format: Option<AccessLogFormat>, context: OwnedAccessLogContext) -> Self {
        Self { format, context, finalized: false }
    }

    fn finalize(&mut self, grpc: &GrpcObservability) {
        if self.finalized {
            return;
        }
        self.finalized = true;

        log_access_event(self.format.as_ref(), self.context.as_borrowed(Some(grpc)));
    }
}

struct GrpcAccessLogBody {
    inner: HttpBody,
    finalizer: GrpcResponseFinalizer,
    grpc: GrpcObservability,
    grpc_web: Option<GrpcWebObservabilityParser>,
    stream_completed: bool,
}

impl GrpcAccessLogBody {
    fn new(inner: HttpBody, finalizer: GrpcResponseFinalizer, grpc: GrpcObservability) -> Self {
        let grpc_web = GrpcWebObservabilityParser::for_protocol(&grpc.protocol);
        Self { inner, finalizer, grpc, grpc_web, stream_completed: false }
    }

    fn observe_frame(&mut self, frame: &Frame<Bytes>) {
        if let Some(trailers) = frame.trailers_ref() {
            self.grpc.update_from_headers(trailers);
        }

        if let Some(data) = frame.data_ref()
            && let Some(parser) = self.grpc_web.as_mut()
        {
            parser.observe_chunk(data, &mut self.grpc);
        }
    }

    fn finish(&mut self) {
        if let Some(parser) = self.grpc_web.as_mut() {
            parser.finish(&mut self.grpc);
        }
        self.finalizer.finalize(&self.grpc);
    }

    fn mark_cancelled_if_dropped_early(&mut self) {
        if self.stream_completed || self.grpc.status.is_some() {
            return;
        }

        self.grpc.status = Some(GrpcStatusCode::Cancelled.as_str().to_string());
        if self.grpc.message.is_none() {
            self.grpc.message = Some("downstream cancelled".to_string());
        }
    }
}

impl Drop for GrpcAccessLogBody {
    fn drop(&mut self) {
        self.mark_cancelled_if_dropped_early();
        self.finish();
    }
}

impl hyper::body::Body for GrpcAccessLogBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match std::pin::Pin::new(&mut self.inner).poll_frame(cx) {
            std::task::Poll::Ready(Some(Ok(frame))) => {
                let stream_completed = frame.is_trailers() || self.inner.is_end_stream();
                self.observe_frame(&frame);
                if stream_completed {
                    self.stream_completed = true;
                    self.finish();
                }
                std::task::Poll::Ready(Some(Ok(frame)))
            }
            std::task::Poll::Ready(Some(Err(error))) => {
                self.stream_completed = true;
                self.finish();
                std::task::Poll::Ready(Some(Err(error)))
            }
            std::task::Poll::Ready(None) => {
                self.stream_completed = true;
                self.finish();
                std::task::Poll::Ready(None)
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

pub(super) fn wrap_grpc_observability_response(
    response: HttpResponse,
    format: Option<AccessLogFormat>,
    context: OwnedAccessLogContext,
    grpc: GrpcObservability,
) -> HttpResponse {
    let (parts, body) = response.into_parts();
    let body = GrpcAccessLogBody::new(body, GrpcResponseFinalizer::new(format, context), grpc)
        .boxed_unsync();
    Response::from_parts(parts, body)
}

#[derive(Debug, Default)]
pub(super) struct GrpcWebObservabilityParser {
    is_text: bool,
    encoded_carryover: BytesMut,
    buffer: BytesMut,
    saw_trailers: bool,
    disabled: bool,
}

impl GrpcWebObservabilityParser {
    pub(super) fn for_protocol(protocol: &str) -> Option<Self> {
        match protocol {
            "grpc-web" => Some(Self { is_text: false, ..Self::default() }),
            "grpc-web-text" => Some(Self { is_text: true, ..Self::default() }),
            _ => None,
        }
    }

    pub(super) fn observe_chunk(&mut self, data: &[u8], grpc: &mut GrpcObservability) {
        if self.disabled {
            return;
        }

        let result = if self.is_text {
            decode_grpc_web_text_observability_chunk(&mut self.encoded_carryover, data).and_then(
                |decoded| {
                    if let Some(decoded) = decoded {
                        self.observe_binary_chunk(&decoded, false, grpc)
                    } else {
                        Ok(())
                    }
                },
            )
        } else {
            self.observe_binary_chunk(data, false, grpc)
        };

        if let Err(error) = result {
            self.disabled = true;
            tracing::debug!(
                protocol = %grpc.protocol,
                service = %grpc.service,
                method = %grpc.method,
                %error,
                "failed to parse grpc-web response trailers for observability"
            );
        }
    }

    pub(super) fn finish(&mut self, grpc: &mut GrpcObservability) {
        if self.disabled {
            return;
        }

        let result = if self.is_text {
            decode_grpc_web_text_observability_final(&mut self.encoded_carryover).and_then(
                |decoded| {
                    if let Some(decoded) = decoded {
                        self.observe_binary_chunk(&decoded, true, grpc)?;
                    } else {
                        self.observe_binary_chunk(&[], true, grpc)?;
                    }
                    Ok(())
                },
            )
        } else {
            self.observe_binary_chunk(&[], true, grpc)
        };

        if let Err(error) = result {
            self.disabled = true;
            tracing::debug!(
                protocol = %grpc.protocol,
                service = %grpc.service,
                method = %grpc.method,
                %error,
                "failed to finish grpc-web response trailer parsing for observability"
            );
        }
    }

    fn observe_binary_chunk(
        &mut self,
        data: &[u8],
        inner_finished: bool,
        grpc: &mut GrpcObservability,
    ) -> Result<(), BoxError> {
        if !data.is_empty() {
            self.buffer.extend_from_slice(data);
        }

        loop {
            let Some(frame) = decode_grpc_web_observability_frame(
                &mut self.buffer,
                inner_finished,
                self.saw_trailers,
            )?
            else {
                return Ok(());
            };

            match frame {
                ParsedGrpcWebObservabilityFrame::Data => {}
                ParsedGrpcWebObservabilityFrame::Trailers(trailers) => {
                    self.saw_trailers = true;
                    grpc.update_from_headers(&trailers);
                }
            }
        }
    }
}

enum ParsedGrpcWebObservabilityFrame {
    Data,
    Trailers(HeaderMap),
}

fn decode_grpc_web_observability_frame(
    buffer: &mut BytesMut,
    inner_finished: bool,
    saw_grpc_web_trailers: bool,
) -> Result<Option<ParsedGrpcWebObservabilityFrame>, BoxError> {
    if buffer.is_empty() {
        return Ok(None);
    }

    if saw_grpc_web_trailers {
        return Err(invalid_grpc_observability("grpc-web response trailer frame must be terminal"));
    }

    if buffer.len() < 5 {
        if inner_finished {
            return Err(invalid_grpc_observability("incomplete grpc-web response frame header"));
        }
        return Ok(None);
    }

    let flags = buffer[0];
    let len = u32::from_be_bytes([buffer[1], buffer[2], buffer[3], buffer[4]]) as usize;
    let frame_len = 5usize.saturating_add(len);
    if buffer.len() < frame_len {
        if inner_finished {
            return Err(invalid_grpc_observability("incomplete grpc-web response frame payload"));
        }
        return Ok(None);
    }

    let frame = buffer.split_to(frame_len).freeze();
    if (flags & 0x80) != 0 {
        Ok(Some(ParsedGrpcWebObservabilityFrame::Trailers(
            decode_grpc_web_trailer_block_for_observability(&frame[5..])?,
        )))
    } else {
        Ok(Some(ParsedGrpcWebObservabilityFrame::Data))
    }
}

fn decode_grpc_web_trailer_block_for_observability(payload: &[u8]) -> Result<HeaderMap, BoxError> {
    let mut trailers = HeaderMap::new();

    for line in payload.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            continue;
        }

        let Some(separator) = line.iter().position(|byte| *byte == b':') else {
            return Err(invalid_grpc_observability("grpc-web trailer line should contain ':'"));
        };
        let (name, value) = line.split_at(separator);
        let value = &value[1..];
        let name = HeaderName::from_bytes(name).map_err(|_| {
            invalid_grpc_observability("grpc-web trailer name should be a valid HTTP header")
        })?;
        let value = std::str::from_utf8(value)
            .map_err(|_| {
                invalid_grpc_observability("grpc-web trailer value should be valid utf-8")
            })?
            .trim();
        let value = HeaderValue::from_str(value).map_err(|_| {
            invalid_grpc_observability("grpc-web trailer value should be a valid HTTP header")
        })?;
        trailers.append(name, value);
    }

    Ok(trailers)
}

fn decode_grpc_web_text_observability_chunk(
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
            invalid_grpc_observability(&format!(
                "invalid grpc-web-text base64 chunk for observability: {error}"
            ))
        })?;
        decoded.extend_from_slice(&chunk);
    }

    Ok((!decoded.is_empty()).then(|| Bytes::from(decoded)))
}

pub(super) fn decode_grpc_web_text_observability_final(
    carryover: &mut BytesMut,
) -> Result<Option<Bytes>, BoxError> {
    if carryover.is_empty() {
        return Ok(None);
    }

    if carryover.len() % 4 != 0 {
        return Err(invalid_grpc_observability("incomplete grpc-web-text base64 response body"));
    }

    let encoded = carryover.split();
    let mut decoded = Vec::new();
    for quantum in encoded.chunks_exact(4) {
        let chunk = STANDARD.decode(quantum).map_err(|error| {
            invalid_grpc_observability(&format!(
                "invalid grpc-web-text base64 chunk for observability: {error}"
            ))
        })?;
        decoded.extend_from_slice(&chunk);
    }

    Ok((!decoded.is_empty()).then(|| Bytes::from(decoded)))
}

fn invalid_grpc_observability(message: &str) -> BoxError {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message).into()
}

fn grpc_service_method(path: &str) -> Option<(&str, &str)> {
    let path = path.strip_prefix('/')?;
    let (service, method) = path.split_once('/')?;
    if service.is_empty() || method.is_empty() || method.contains('/') {
        return None;
    }
    Some((service, method))
}

fn split_header_content_type(content_type: &str) -> (&str, &str) {
    let mut parts = content_type.splitn(2, ';');
    let mime = parts.next().unwrap_or_default().trim();
    let params = parts.next().unwrap_or_default().trim();
    (mime, params)
}
