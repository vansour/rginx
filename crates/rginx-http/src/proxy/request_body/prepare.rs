use super::*;

use super::replay::{ReplayableRequestBody, is_idempotent_method};
use super::streaming::relay_streaming_request_body;

struct CollectedRequestBody {
    body: Bytes,
    trailers: Option<HeaderMap>,
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

impl PreparedProxyRequest {
    pub(in crate::proxy) async fn from_request(
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

    pub(in crate::proxy) fn can_failover(&self) -> bool {
        self.peer_failover_enabled
            && is_idempotent_method(&self.method)
            && matches!(self.body, PreparedRequestBody::Replayable { .. })
    }

    pub(in crate::proxy) fn build_for_peer(
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
