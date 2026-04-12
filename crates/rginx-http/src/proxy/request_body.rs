use super::grpc_web::{GrpcWebMode, GrpcWebRequestBody, GrpcWebTextDecodeBody};
use super::*;

pub(super) struct PreparedProxyRequest {
    pub method: Method,
    pub uri: Uri,
    pub headers: HeaderMap,
    pub body: PreparedRequestBody,
}

pub(super) enum PreparedRequestBody {
    Replayable { body: Bytes, trailers: Option<HeaderMap> },
    Streaming(Option<HttpBody>),
}

struct CollectedRequestBody {
    body: Bytes,
    trailers: Option<HeaderMap>,
}

#[derive(Debug)]
pub(super) enum PrepareRequestError {
    PayloadTooLarge { max_request_body_bytes: usize },
    Other(Box<dyn std::error::Error + Send + Sync>),
}

impl PrepareRequestError {
    fn payload_too_large(max_request_body_bytes: usize) -> Self {
        Self::PayloadTooLarge { max_request_body_bytes }
    }

    fn boxed(error: BoxError) -> Self {
        Self::Other(error)
    }
}

impl std::fmt::Display for PrepareRequestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PayloadTooLarge { max_request_body_bytes } => write!(
                formatter,
                "request body exceeded configured limit of {max_request_body_bytes} bytes"
            ),
            Self::Other(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for PrepareRequestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::PayloadTooLarge { .. } => None,
            Self::Other(error) => Some(error.as_ref()),
        }
    }
}

struct ReplayableRequestBody {
    body: Option<Bytes>,
    trailers: Option<HeaderMap>,
}

impl ReplayableRequestBody {
    fn new(body: Bytes, trailers: Option<HeaderMap>) -> Self {
        Self { body: Some(body), trailers }
    }
}

impl hyper::body::Body for ReplayableRequestBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();

        if let Some(body) = this.body.take()
            && !body.is_empty()
        {
            return std::task::Poll::Ready(Some(Ok(Frame::data(body))));
        }

        if let Some(trailers) = this.trailers.take() {
            return std::task::Poll::Ready(Some(Ok(Frame::trailers(trailers))));
        }

        std::task::Poll::Ready(None)
    }

    fn is_end_stream(&self) -> bool {
        self.body.as_ref().is_none_or(Bytes::is_empty) && self.trailers.is_none()
    }

    fn size_hint(&self) -> SizeHint {
        let mut hint = SizeHint::new();
        hint.set_exact(self.body.as_ref().map_or(0, |body| body.len() as u64));
        hint
    }
}

impl PreparedProxyRequest {
    pub(super) async fn from_request(
        request: Request<HttpBody>,
        upstream_name: &str,
        request_body_read_timeout: Option<Duration>,
        write_timeout: Duration,
        max_replayable_request_body_bytes: usize,
        max_request_body_bytes: Option<usize>,
        grpc_web_mode: Option<&GrpcWebMode>,
    ) -> Result<Self, PrepareRequestError> {
        let (parts, body) = request.into_parts();
        let body_timeout = request_body_read_timeout.unwrap_or(write_timeout);
        let replayable = prepare_request_body(
            upstream_name,
            &parts.method,
            &parts.headers,
            body,
            body_timeout,
            max_replayable_request_body_bytes,
            max_request_body_bytes,
            grpc_web_mode,
        )
        .await?;

        Ok(Self { method: parts.method, uri: parts.uri, headers: parts.headers, body: replayable })
    }

    pub(super) fn can_failover(&self) -> bool {
        is_idempotent_method(&self.method)
            && matches!(self.body, PreparedRequestBody::Replayable { .. })
    }

    pub(super) fn build_for_peer(
        &mut self,
        peer: &UpstreamPeer,
        target: &ProxyTarget,
        client_address: &ClientAddress,
        forwarded_proto: &str,
        grpc_web_mode: Option<&GrpcWebMode>,
    ) -> Result<Request<HttpBody>, Box<dyn std::error::Error + Send + Sync>> {
        let original_host = self.headers.get(HOST).cloned();
        let mut headers = self.headers.clone();
        let uri = build_proxy_uri(peer, &self.uri, target.strip_prefix.as_deref())?;
        sanitize_request_headers(
            &mut headers,
            &peer.authority,
            original_host,
            client_address,
            forwarded_proto,
            target.preserve_host,
            &target.proxy_set_headers,
            grpc_web_mode,
        )?;

        tracing::debug!(
            upstream = %target.upstream.name,
            peer = %peer.url,
            uri = %uri,
            "forwarding request to upstream"
        );

        let mut request = Request::new(match &mut self.body {
            PreparedRequestBody::Replayable { body, trailers } => {
                ReplayableRequestBody::new(body.clone(), trailers.clone()).boxed_unsync()
            }
            PreparedRequestBody::Streaming(body) => body.take().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "streaming request body is no longer available for replay",
                )
            })?,
        });
        *request.method_mut() = self.method.clone();
        *request.version_mut() = upstream_request_version(target.upstream.protocol);
        *request.uri_mut() = uri;
        *request.headers_mut() = headers;
        Ok(request)
    }
}

async fn prepare_request_body(
    upstream_name: &str,
    method: &Method,
    headers: &HeaderMap,
    body: HttpBody,
    body_timeout: Duration,
    max_replayable_request_body_bytes: usize,
    max_request_body_bytes: Option<usize>,
    grpc_web_mode: Option<&GrpcWebMode>,
) -> Result<PreparedRequestBody, PrepareRequestError> {
    let body_timeout_label = format!("upstream `{upstream_name}` request body");

    if let Some(max_request_body_bytes) = max_request_body_bytes {
        if body.is_end_stream() {
            return Ok(PreparedRequestBody::Replayable { body: Bytes::new(), trailers: None });
        }

        if body.size_hint().lower() > max_request_body_bytes as u64 {
            return Err(PrepareRequestError::payload_too_large(max_request_body_bytes));
        }

        let body = collect_request_body(
            downstream_request_body(body, body_timeout, body_timeout_label.clone(), grpc_web_mode),
            Some(max_request_body_bytes),
        )
        .await?;
        return Ok(PreparedRequestBody::Replayable { body: body.body, trailers: body.trailers });
    }

    if preserved_te_trailers_value(headers).is_some() {
        return Ok(PreparedRequestBody::Streaming(Some(downstream_request_body(
            body,
            body_timeout,
            body_timeout_label,
            grpc_web_mode,
        ))));
    }

    if !is_idempotent_method(method) {
        return Ok(PreparedRequestBody::Streaming(Some(downstream_request_body(
            body,
            body_timeout,
            body_timeout_label,
            grpc_web_mode,
        ))));
    }

    if body.is_end_stream() {
        return Ok(PreparedRequestBody::Replayable { body: Bytes::new(), trailers: None });
    }

    match body.size_hint().upper() {
        Some(upper) if upper <= max_replayable_request_body_bytes as u64 => {
            let body = collect_request_body(
                downstream_request_body(body, body_timeout, body_timeout_label, grpc_web_mode),
                None,
            )
            .await?;
            Ok(PreparedRequestBody::Replayable { body: body.body, trailers: body.trailers })
        }
        _ => Ok(PreparedRequestBody::Streaming(Some(downstream_request_body(
            body,
            body_timeout,
            body_timeout_label,
            grpc_web_mode,
        )))),
    }
}

async fn collect_request_body<B>(
    mut body: B,
    max_request_body_bytes: Option<usize>,
) -> Result<CollectedRequestBody, PrepareRequestError>
where
    B: hyper::body::Body<Data = Bytes> + Unpin,
    B::Error: Into<BoxError>,
{
    let mut collected = BytesMut::new();
    let mut trailers = None;

    while let Some(frame) = body.frame().await {
        let frame =
            frame.map_err(|error| PrepareRequestError::boxed(Into::<BoxError>::into(error)))?;
        let frame = match frame.into_data() {
            Ok(data) => {
                if let Some(max_request_body_bytes) = max_request_body_bytes
                    && data.len() > max_request_body_bytes.saturating_sub(collected.len())
                {
                    return Err(PrepareRequestError::payload_too_large(max_request_body_bytes));
                }
                collected.extend_from_slice(&data);
                continue;
            }
            Err(frame) => frame,
        };

        let frame_trailers = match frame.into_trailers() {
            Ok(trailers) => trailers,
            Err(_) => continue,
        };
        if let Some(existing) = trailers.as_mut() {
            append_header_map(existing, &frame_trailers);
        } else {
            trailers = Some(frame_trailers);
        }
    }

    Ok(CollectedRequestBody { body: collected.freeze(), trailers })
}

fn downstream_request_body(
    body: HttpBody,
    body_timeout: Duration,
    label: String,
    grpc_web_mode: Option<&GrpcWebMode>,
) -> HttpBody {
    let body = IdleTimeoutBody::new(body, body_timeout, label);
    match grpc_web_mode {
        Some(mode) if mode.is_text() => {
            GrpcWebRequestBody::new(GrpcWebTextDecodeBody::new(body)).boxed_unsync()
        }
        Some(_) => GrpcWebRequestBody::new(body).boxed_unsync(),
        None => body.boxed_unsync(),
    }
}

pub(super) fn is_idempotent_method(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::PUT | Method::DELETE | Method::OPTIONS | Method::TRACE
    )
}

pub(super) fn can_retry_peer_request(
    prepared_request: &PreparedProxyRequest,
    peers: &[UpstreamPeer],
    attempt_index: usize,
) -> bool {
    prepared_request.can_failover() && attempt_index + 1 < peers.len()
}
