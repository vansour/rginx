use super::grpc_web::{GrpcWebMode, GrpcWebRequestBody, GrpcWebTextDecodeBody};
use super::*;
use crate::handler::boxed_body;
use std::error::Error as StdError;

pub(super) struct PreparedProxyRequest {
    pub method: Method,
    pub uri: Uri,
    pub headers: HeaderMap,
    pub body: PreparedRequestBody,
    pub(super) peer_failover_enabled: bool,
    pub(super) wait_for_streaming_body: bool,
}

pub(super) enum PreparedRequestBody {
    Replayable { body: Bytes, trailers: Option<HeaderMap> },
    Streaming(Option<HttpBody>),
}

struct CollectedRequestBody {
    body: Bytes,
    trailers: Option<HeaderMap>,
}

pub(super) struct BuiltUpstreamRequest {
    pub request: Request<HttpBody>,
    pub body_completion: Option<StreamingBodyCompletion>,
}

pub(super) type StreamingBodyCompletion = tokio::sync::oneshot::Receiver<Result<(), BoxError>>;

#[derive(Debug)]
struct RelayedRequestBody {
    receiver: tokio::sync::mpsc::Receiver<Result<Frame<Bytes>, BoxError>>,
    done: bool,
}

struct PrepareRequestBodyConfig<'a> {
    upstream_name: &'a str,
    method: &'a Method,
    headers: &'a HeaderMap,
    body_timeout: Duration,
    max_replayable_request_body_bytes: usize,
    max_request_body_bytes: Option<usize>,
    request_buffering: RouteBufferingPolicy,
    grpc_web_mode: Option<&'a GrpcWebMode>,
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
        if let Some(max_request_body_bytes) = request_body_limit_error(error.as_ref()) {
            Self::payload_too_large(max_request_body_bytes)
        } else {
            Self::Other(error)
        }
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

impl RelayedRequestBody {
    fn new(receiver: tokio::sync::mpsc::Receiver<Result<Frame<Bytes>, BoxError>>) -> Self {
        Self { receiver, done: false }
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

impl hyper::body::Body for RelayedRequestBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if self.done {
            return std::task::Poll::Ready(None);
        }

        match self.receiver.poll_recv(cx) {
            std::task::Poll::Ready(Some(Ok(frame))) => {
                if frame.is_trailers() {
                    self.done = true;
                }
                std::task::Poll::Ready(Some(Ok(frame)))
            }
            std::task::Poll::Ready(Some(Err(error))) => {
                self.done = true;
                std::task::Poll::Ready(Some(Err(error)))
            }
            std::task::Poll::Ready(None) => {
                self.done = true;
                std::task::Poll::Ready(None)
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.done
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
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
        request_buffering: RouteBufferingPolicy,
        grpc_web_mode: Option<&GrpcWebMode>,
    ) -> Result<Self, PrepareRequestError> {
        let (parts, body) = request.into_parts();
        let body_timeout = request_body_read_timeout.unwrap_or(write_timeout);
        let prepared_body = prepare_request_body(
            body,
            PrepareRequestBodyConfig {
                upstream_name,
                method: &parts.method,
                headers: &parts.headers,
                body_timeout,
                max_replayable_request_body_bytes,
                max_request_body_bytes,
                request_buffering,
                grpc_web_mode,
            },
        )
        .await?;

        Ok(Self {
            method: parts.method,
            uri: parts.uri,
            headers: parts.headers,
            body: prepared_body,
            peer_failover_enabled: request_buffering != RouteBufferingPolicy::Off,
            wait_for_streaming_body: max_request_body_bytes.is_some(),
        })
    }

    pub(super) fn can_failover(&self) -> bool {
        self.peer_failover_enabled
            && is_idempotent_method(&self.method)
            && matches!(self.body, PreparedRequestBody::Replayable { .. })
    }

    pub(super) fn build_for_peer(
        &mut self,
        peer: &ResolvedUpstreamPeer,
        target: &ProxyTarget,
        client_address: &ClientAddress,
        forwarded_proto: &str,
        grpc_web_mode: Option<&GrpcWebMode>,
    ) -> Result<BuiltUpstreamRequest, Box<dyn std::error::Error + Send + Sync>> {
        let original_host = self.headers.get(HOST).cloned();
        let mut headers = self.headers.clone();
        let uri = build_proxy_uri(peer, &self.uri, target.strip_prefix.as_deref())?;
        sanitize_request_headers(
            &mut headers,
            &peer.upstream_authority,
            original_host,
            client_address,
            forwarded_proto,
            target.preserve_host,
            &target.proxy_set_headers,
            grpc_web_mode,
        )?;

        tracing::debug!(
            upstream = %target.upstream.name,
            peer = %peer.display_url,
            uri = %uri,
            "forwarding request to upstream"
        );

        let (request_body, body_completion) = match &mut self.body {
            PreparedRequestBody::Replayable { body, trailers } => {
                (ReplayableRequestBody::new(body.clone(), trailers.clone()).boxed_unsync(), None)
            }
            PreparedRequestBody::Streaming(body) => {
                let body = body.take().ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "streaming request body is no longer available for replay",
                    )
                })?;
                if self.wait_for_streaming_body {
                    let (body, completion) = relay_streaming_request_body(body);
                    (body, Some(completion))
                } else {
                    (body, None)
                }
            }
        };
        let mut request = Request::new(request_body);
        *request.method_mut() = self.method.clone();
        *request.version_mut() = upstream_request_version(target.upstream.protocol);
        *request.uri_mut() = uri;
        *request.headers_mut() = headers;
        Ok(BuiltUpstreamRequest { request, body_completion })
    }
}

fn relay_streaming_request_body(body: HttpBody) -> (HttpBody, StreamingBodyCompletion) {
    let (frame_tx, frame_rx) = tokio::sync::mpsc::channel(1);
    let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        let mut body = body;
        let result = loop {
            match body.frame().await {
                Some(Ok(frame)) => {
                    let reached_end = frame.is_trailers();
                    let _ = frame_tx.send(Ok(frame)).await;
                    if reached_end {
                        break Ok(());
                    }
                }
                Some(Err(error)) => {
                    let _ = frame_tx.send(Err(clone_box_error(error.as_ref()))).await;
                    break Err(clone_box_error(error.as_ref()));
                }
                None => break Ok(()),
            }
        };

        drop(frame_tx);
        let _ = completion_tx.send(result);
    });

    (boxed_body(RelayedRequestBody::new(frame_rx)), completion_rx)
}

fn clone_box_error(error: &(dyn StdError + 'static)) -> BoxError {
    if let Some(max_request_body_bytes) = request_body_limit_error(error) {
        return Box::new(crate::timeout::RequestBodyLimitError::new(max_request_body_bytes));
    }

    if let Some(io_error) = error.downcast_ref::<std::io::Error>() {
        return Box::new(std::io::Error::new(io_error.kind(), io_error.to_string()));
    }

    Box::new(std::io::Error::other(error.to_string()))
}

async fn prepare_request_body(
    body: HttpBody,
    config: PrepareRequestBodyConfig<'_>,
) -> Result<PreparedRequestBody, PrepareRequestError> {
    let body_timeout_label = format!("upstream `{}` request body", config.upstream_name);
    let preserves_trailers = preserved_te_trailers_value(config.headers).is_some();

    if let Some(max_request_body_bytes) = config.max_request_body_bytes
        && body.size_hint().lower() > max_request_body_bytes as u64
    {
        return Err(PrepareRequestError::payload_too_large(max_request_body_bytes));
    }

    let body = downstream_request_body(
        body,
        config.body_timeout,
        body_timeout_label,
        config.grpc_web_mode,
        config.max_request_body_bytes,
    );

    if body.is_end_stream() {
        return Ok(match config.request_buffering {
            RouteBufferingPolicy::Off => PreparedRequestBody::Streaming(Some(body)),
            RouteBufferingPolicy::On | RouteBufferingPolicy::Auto => {
                PreparedRequestBody::Replayable { body: Bytes::new(), trailers: None }
            }
        });
    }

    if config.grpc_web_mode.is_some() {
        // grpc-web translation must validate the full downstream payload before
        // we open an upstream request, otherwise malformed bodies can be masked
        // by upstream connection failures.
        let body = collect_request_body(body).await?;
        return Ok(PreparedRequestBody::Replayable { body: body.body, trailers: body.trailers });
    }

    match config.request_buffering {
        RouteBufferingPolicy::Off => Ok(PreparedRequestBody::Streaming(Some(body))),
        RouteBufferingPolicy::On if !preserves_trailers => {
            let body = collect_request_body(body).await?;
            Ok(PreparedRequestBody::Replayable { body: body.body, trailers: body.trailers })
        }
        RouteBufferingPolicy::On => Ok(PreparedRequestBody::Streaming(Some(body))),
        RouteBufferingPolicy::Auto
            if !preserves_trailers
                && is_idempotent_method(config.method)
                && matches!(
                    body.size_hint().upper(),
                    Some(upper) if upper <= config.max_replayable_request_body_bytes as u64
                ) =>
        {
            let body = collect_request_body(body).await?;
            Ok(PreparedRequestBody::Replayable { body: body.body, trailers: body.trailers })
        }
        RouteBufferingPolicy::Auto => Ok(PreparedRequestBody::Streaming(Some(body))),
    }
}

async fn collect_request_body(
    mut body: HttpBody,
) -> Result<CollectedRequestBody, PrepareRequestError> {
    let mut collected = BytesMut::new();
    let mut trailers = None;

    while let Some(frame) = body.frame().await {
        let frame =
            frame.map_err(|error| PrepareRequestError::boxed(Into::<BoxError>::into(error)))?;
        let frame = match frame.into_data() {
            Ok(data) => {
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
    max_request_body_bytes: Option<usize>,
) -> HttpBody {
    let body = IdleTimeoutBody::new(body, body_timeout, label);
    let body = match grpc_web_mode {
        Some(mode) if mode.is_text() => {
            GrpcWebRequestBody::new(GrpcWebTextDecodeBody::new(body)).boxed_unsync()
        }
        Some(_) => GrpcWebRequestBody::new(body).boxed_unsync(),
        None => body.boxed_unsync(),
    };

    match max_request_body_bytes {
        Some(max_request_body_bytes) => {
            MaxBytesBody::new(body, max_request_body_bytes).boxed_unsync()
        }
        None => body,
    }
}

pub(super) fn request_body_limit_error(error: &(dyn std::error::Error + 'static)) -> Option<usize> {
    let mut current = Some(error);
    while let Some(candidate) = current {
        if let Some(limit_error) = candidate.downcast_ref::<RequestBodyLimitError>() {
            return Some(limit_error.max_request_body_bytes());
        }

        current = candidate.source();
    }

    None
}

pub(super) fn is_idempotent_method(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::PUT | Method::DELETE | Method::OPTIONS | Method::TRACE
    )
}

pub(super) fn can_retry_peer_request(
    prepared_request: &PreparedProxyRequest,
    peer_count: usize,
    attempt_index: usize,
) -> bool {
    prepared_request.can_failover() && attempt_index + 1 < peer_count
}

#[cfg(test)]
mod tests;
