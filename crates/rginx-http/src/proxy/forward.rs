use super::grpc_web::{
    GrpcWebEncoding, GrpcWebMode, GrpcWebResponseBody, GrpcWebTextEncodeBody,
    extract_grpc_initial_trailers,
};
use super::health::{ActivePeerBody, ActivePeerGuard};
use super::request_body::{PrepareRequestError, PreparedProxyRequest, can_retry_peer_request};
use super::upgrade::proxy_upgraded_connection;
use super::*;

#[derive(Debug, Clone, Copy)]
pub struct DownstreamRequestOptions {
    pub request_body_read_timeout: Option<Duration>,
    pub max_request_body_bytes: Option<usize>,
}

#[derive(Debug, Clone)]
struct GrpcResponseDeadline {
    pub deadline: TokioInstant,
    pub timeout: Duration,
    pub timeout_message: String,
}

pub async fn forward_request(
    state: SharedState,
    clients: ProxyClients,
    mut request: Request<Incoming>,
    target: &ProxyTarget,
    client_address: ClientAddress,
    downstream_proto: &str,
    downstream: DownstreamRequestOptions,
    request_id: &str,
) -> HttpResponse {
    let request_headers = request.headers().clone();
    let grpc_web_mode = match detect_grpc_web_mode(request.headers()) {
        Ok(mode) => mode,
        Err(message) => {
            return unsupported_media_type(&request_headers, format!("{message}\n"));
        }
    };
    let upstream_request_timeout =
        match effective_upstream_request_timeout(&request_headers, target.upstream.request_timeout)
        {
            Ok(timeout) => timeout,
            Err(message) => return bad_request(&request_headers, format!("{message}\n")),
        };
    let client = match clients.for_upstream(target.upstream.as_ref()) {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(
                request_id = %request_id,
                upstream = %target.upstream_name,
                %error,
                "failed to select proxy client"
            );
            return bad_gateway(
                &request_headers,
                format!("upstream `{}` TLS client is unavailable\n", target.upstream_name),
            );
        }
    };

    let downstream_upgrade = if is_upgrade_request(request.version(), request.headers()) {
        Some(hyper::upgrade::on(&mut request))
    } else {
        None
    };

    let mut prepared_request = match PreparedProxyRequest::from_request(
        request,
        &target.upstream_name,
        downstream.request_body_read_timeout,
        target.upstream.write_timeout,
        target.upstream.max_replayable_request_body_bytes,
        downstream.max_request_body_bytes,
        grpc_web_mode.as_ref(),
    )
    .await
    {
        Ok(request) => request,
        Err(PrepareRequestError::PayloadTooLarge { max_request_body_bytes }) => {
            tracing::info!(
                request_id = %request_id,
                upstream = %target.upstream_name,
                max_request_body_bytes,
                "rejecting request body that exceeds configured server limit"
            );
            return payload_too_large(
                &request_headers,
                format!(
                    "request body exceeds server.max_request_body_bytes ({max_request_body_bytes} bytes)\n"
                ),
            );
        }
        Err(error) => {
            tracing::warn!(
                request_id = %request_id,
                upstream = %target.upstream_name,
                %error,
                "failed to prepare upstream request"
            );
            return bad_gateway(
                &request_headers,
                format!("failed to prepare upstream request for `{}`\n", target.upstream_name),
            );
        }
    };
    let can_failover = prepared_request.can_failover();
    let selected = clients.select_peers(
        target.upstream.as_ref(),
        client_address.client_ip,
        if can_failover { MAX_FAILOVER_ATTEMPTS } else { 1 },
    );
    let peers = selected.peers;
    if peers.is_empty() {
        tracing::warn!(
            request_id = %request_id,
            upstream = %target.upstream_name,
            skipped_unhealthy = selected.skipped_unhealthy,
            "proxy route has no healthy peers available"
        );
        return bad_gateway(
            &request_headers,
            format!("upstream `{}` has no healthy peers available\n", target.upstream_name),
        );
    }

    for (attempt_index, peer) in peers.iter().enumerate() {
        let grpc_response_deadline =
            grpc_protocol_request(&request_headers).then(|| GrpcResponseDeadline {
                deadline: TokioInstant::now() + upstream_request_timeout,
                timeout: upstream_request_timeout,
                timeout_message: grpc_timeout_message(
                    &target.upstream_name,
                    upstream_request_timeout,
                ),
            });
        let upstream_request = match prepared_request.build_for_peer(
            peer,
            target,
            &client_address,
            downstream_proto,
            grpc_web_mode.as_ref(),
        ) {
            Ok(request) => request,
            Err(error) => {
                tracing::warn!(
                    request_id = %request_id,
                    upstream = %target.upstream_name,
                    peer = %peer.url,
                    %error,
                    "failed to build upstream request"
                );
                return bad_gateway(
                    &request_headers,
                    format!("failed to build upstream request for `{}`\n", target.upstream_name),
                );
            }
        };
        let active_peer = clients.track_active_request(&target.upstream_name, &peer.url);

        match wait_for_upstream_stage(
            upstream_request_timeout,
            &target.upstream_name,
            "request",
            client.request(upstream_request),
        )
        .await
        {
            Ok(Ok(mut response)) => {
                let recovered = clients.record_peer_success(&target.upstream_name, &peer.url);
                if recovered {
                    tracing::info!(
                        upstream = %target.upstream_name,
                        peer = %peer.url,
                        "upstream peer recovered from passive health check cooldown"
                    );
                }
                if attempt_index > 0 {
                    tracing::info!(
                        request_id = %request_id,
                        upstream = %target.upstream_name,
                        peer = %peer.url,
                        attempt = attempt_index + 1,
                        "upstream failover request succeeded"
                    );
                }

                let upstream_upgrade = if downstream_upgrade.is_some()
                    && is_upgrade_response(response.status(), response.headers())
                {
                    Some(hyper::upgrade::on(&mut response))
                } else {
                    None
                };

                if let (Some(downstream_upgrade), Some(upstream_upgrade)) =
                    (downstream_upgrade.clone(), upstream_upgrade)
                {
                    state.spawn_background_task(proxy_upgraded_connection(
                        downstream_upgrade,
                        upstream_upgrade,
                        target.upstream_name.clone(),
                        peer.url.clone(),
                        active_peer,
                    ));
                    return build_downstream_response(
                        response,
                        &target.upstream_name,
                        &peer.url,
                        target.upstream.idle_timeout,
                        grpc_response_deadline.clone(),
                        grpc_web_mode.as_ref(),
                        None,
                    );
                }

                return build_downstream_response(
                    response,
                    &target.upstream_name,
                    &peer.url,
                    target.upstream.idle_timeout,
                    grpc_response_deadline,
                    grpc_web_mode.as_ref(),
                    Some(active_peer),
                );
            }
            Ok(Err(error)) if can_retry_peer_request(&prepared_request, &peers, attempt_index) => {
                let failure = clients.record_peer_failure(&target.upstream_name, &peer.url);
                let next_peer = &peers[attempt_index + 1];
                tracing::warn!(
                    request_id = %request_id,
                    upstream = %target.upstream_name,
                    failed_peer = %peer.url,
                    next_peer = %next_peer.url,
                    attempt = attempt_index + 1,
                    consecutive_failures = failure.consecutive_failures,
                    entered_cooldown = failure.entered_cooldown,
                    %error,
                    "retrying idempotent upstream request on alternate peer"
                );
            }
            Ok(Err(error)) => {
                let failure = clients.record_peer_failure(&target.upstream_name, &peer.url);
                tracing::warn!(
                    request_id = %request_id,
                    upstream = %target.upstream_name,
                    peer = %peer.url,
                    consecutive_failures = failure.consecutive_failures,
                    entered_cooldown = failure.entered_cooldown,
                    %error,
                    "upstream request failed"
                );
                return bad_gateway(
                    &request_headers,
                    format!("upstream `{}` is unavailable\n", target.upstream_name),
                );
            }
            Err(error) if can_retry_peer_request(&prepared_request, &peers, attempt_index) => {
                let failure = clients.record_peer_failure(&target.upstream_name, &peer.url);
                let next_peer = &peers[attempt_index + 1];
                tracing::warn!(
                    request_id = %request_id,
                    upstream = %target.upstream_name,
                    failed_peer = %peer.url,
                    next_peer = %next_peer.url,
                    attempt = attempt_index + 1,
                    timeout_ms = upstream_request_timeout.as_millis() as u64,
                    consecutive_failures = failure.consecutive_failures,
                    entered_cooldown = failure.entered_cooldown,
                    %error,
                    "retrying idempotent upstream request on alternate peer after timeout"
                );
            }
            Err(error) => {
                let failure = clients.record_peer_failure(&target.upstream_name, &peer.url);
                tracing::warn!(
                    request_id = %request_id,
                    upstream = %target.upstream_name,
                    peer = %peer.url,
                    timeout_ms = upstream_request_timeout.as_millis() as u64,
                    consecutive_failures = failure.consecutive_failures,
                    entered_cooldown = failure.entered_cooldown,
                    %error,
                    "upstream request timed out"
                );
                return gateway_timeout(
                    &request_headers,
                    format!(
                        "{}\n",
                        grpc_timeout_message(&target.upstream_name, upstream_request_timeout)
                    ),
                );
            }
        }
    }

    bad_gateway(&request_headers, format!("upstream `{}` is unavailable\n", target.upstream_name))
}

pub(super) async fn wait_for_upstream_stage<T>(
    request_timeout: Duration,
    upstream_name: &str,
    stage: &str,
    future: impl Future<Output = T>,
) -> Result<T, Error> {
    tokio::time::timeout(request_timeout, future).await.map_err(|_| {
        Error::Server(format!(
            "upstream `{upstream_name}` {stage} timed out after {} ms",
            request_timeout.as_millis()
        ))
    })
}

fn gateway_timeout(request_headers: &HeaderMap, message: String) -> HttpResponse {
    grpc_error_response(request_headers, GrpcStatusCode::DeadlineExceeded, &message).unwrap_or_else(
        || {
            crate::handler::text_response(
                StatusCode::GATEWAY_TIMEOUT,
                "text/plain; charset=utf-8",
                message,
            )
        },
    )
}

fn grpc_timeout_message(upstream_name: &str, timeout: Duration) -> String {
    format!("upstream `{upstream_name}` timed out after {} ms", timeout.as_millis())
}

fn bad_gateway(request_headers: &HeaderMap, message: String) -> HttpResponse {
    grpc_error_response(request_headers, GrpcStatusCode::Unavailable, &message).unwrap_or_else(
        || {
            crate::handler::text_response(
                StatusCode::BAD_GATEWAY,
                "text/plain; charset=utf-8",
                message,
            )
        },
    )
}

fn payload_too_large(request_headers: &HeaderMap, message: String) -> HttpResponse {
    grpc_error_response(request_headers, GrpcStatusCode::ResourceExhausted, &message)
        .unwrap_or_else(|| {
            crate::handler::text_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "text/plain; charset=utf-8",
                message,
            )
        })
}

fn bad_request(request_headers: &HeaderMap, message: String) -> HttpResponse {
    grpc_error_response(request_headers, GrpcStatusCode::InvalidArgument, &message).unwrap_or_else(
        || {
            crate::handler::text_response(
                StatusCode::BAD_REQUEST,
                "text/plain; charset=utf-8",
                message,
            )
        },
    )
}

fn unsupported_media_type(request_headers: &HeaderMap, message: String) -> HttpResponse {
    grpc_error_response(request_headers, GrpcStatusCode::InvalidArgument, &message).unwrap_or_else(
        || {
            crate::handler::text_response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "text/plain; charset=utf-8",
                message,
            )
        },
    )
}

pub(super) fn detect_grpc_web_mode(
    headers: &HeaderMap,
) -> Result<Option<GrpcWebMode>, &'static str> {
    let Some(content_type) = headers.get(CONTENT_TYPE) else {
        return Ok(None);
    };
    let Ok(content_type_str) = content_type.to_str() else {
        return Ok(None);
    };

    let (mime, params) = split_content_type(content_type_str);
    let normalized_mime = mime.to_ascii_lowercase();
    if !normalized_mime.starts_with(GRPC_WEB_CONTENT_TYPE_PREFIX) {
        return Ok(None);
    }

    let (encoding, upstream_mime) =
        if normalized_mime.starts_with(GRPC_WEB_TEXT_CONTENT_TYPE_PREFIX) {
            (
                GrpcWebEncoding::Text,
                normalized_mime.replacen(
                    GRPC_WEB_TEXT_CONTENT_TYPE_PREFIX,
                    GRPC_CONTENT_TYPE_PREFIX,
                    1,
                ),
            )
        } else {
            (
                GrpcWebEncoding::Binary,
                normalized_mime.replacen(GRPC_WEB_CONTENT_TYPE_PREFIX, GRPC_CONTENT_TYPE_PREFIX, 1),
            )
        };
    let upstream_content_type =
        if params.is_empty() { upstream_mime } else { format!("{upstream_mime}; {params}") };

    let upstream_content_type = HeaderValue::from_str(&upstream_content_type)
        .map_err(|_| "invalid grpc-web content-type")?;

    Ok(Some(GrpcWebMode {
        downstream_content_type: content_type.clone(),
        upstream_content_type,
        encoding,
    }))
}

fn grpc_protocol_request(headers: &HeaderMap) -> bool {
    let Some(content_type) = headers.get(CONTENT_TYPE) else {
        return false;
    };
    let Ok(content_type) = content_type.to_str() else {
        return false;
    };
    let (mime, _) = split_content_type(content_type);
    mime.to_ascii_lowercase().starts_with(GRPC_CONTENT_TYPE_PREFIX)
}

pub(super) fn effective_upstream_request_timeout(
    headers: &HeaderMap,
    upstream_timeout: Duration,
) -> Result<Duration, String> {
    let grpc_timeout = parse_grpc_timeout(headers)?;
    Ok(grpc_timeout.map_or(upstream_timeout, |timeout| timeout.min(upstream_timeout)))
}

pub(super) fn parse_grpc_timeout(headers: &HeaderMap) -> Result<Option<Duration>, String> {
    if !grpc_protocol_request(headers) {
        return Ok(None);
    }

    let Some(timeout) = headers.get(GRPC_TIMEOUT_HEADER) else {
        return Ok(None);
    };
    let value = timeout
        .to_str()
        .map_err(|_| format!("invalid {GRPC_TIMEOUT_HEADER} header: expected ASCII"))?;
    let value = value.trim();
    if value.len() < 2 {
        return Err(format!(
            "invalid {GRPC_TIMEOUT_HEADER} header: expected 1-8 ASCII digits followed by H/M/S/m/u/n"
        ));
    }

    let (amount, unit) = value.split_at(value.len() - 1);
    if amount.is_empty()
        || amount.len() > MAX_GRPC_TIMEOUT_DIGITS
        || !amount.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(format!(
            "invalid {GRPC_TIMEOUT_HEADER} header: expected 1-8 ASCII digits followed by H/M/S/m/u/n"
        ));
    }

    let amount = amount.parse::<u64>().map_err(|_| {
        format!("invalid {GRPC_TIMEOUT_HEADER} header: timeout value is out of range")
    })?;
    Ok(Some(grpc_timeout_duration(amount, unit).ok_or_else(|| {
        format!("invalid {GRPC_TIMEOUT_HEADER} header: expected 1-8 ASCII digits followed by H/M/S/m/u/n")
    })?))
}

fn grpc_timeout_duration(amount: u64, unit: &str) -> Option<Duration> {
    match unit {
        "H" => amount.checked_mul(60 * 60).map(Duration::from_secs),
        "M" => amount.checked_mul(60).map(Duration::from_secs),
        "S" => Some(Duration::from_secs(amount)),
        "m" => Some(Duration::from_millis(amount)),
        "u" => Some(Duration::from_micros(amount)),
        "n" => Some(Duration::from_nanos(amount)),
        _ => None,
    }
}

fn build_downstream_response(
    response: Response<Incoming>,
    upstream_name: &str,
    peer_url: &str,
    idle_timeout: Duration,
    grpc_response_deadline: Option<GrpcResponseDeadline>,
    grpc_web_mode: Option<&GrpcWebMode>,
    active_peer: Option<ActivePeerGuard>,
) -> HttpResponse {
    let (mut parts, body) = response.into_parts();
    let preserve_upgrade =
        grpc_web_mode.is_none() && is_upgrade_response(parts.status, &parts.headers);
    sanitize_response_headers(&mut parts.headers, preserve_upgrade);

    let label = format!("upstream `{upstream_name}` response body from `{peer_url}`");
    let body = if preserve_upgrade {
        full_body(Bytes::new())
    } else if let Some(grpc_web_mode) = grpc_web_mode {
        let fallback_trailers = extract_grpc_initial_trailers(&mut parts.headers);
        parts.headers.insert(CONTENT_TYPE, grpc_web_mode.downstream_content_type.clone());
        parts.headers.remove(CONTENT_LENGTH);
        let body = IdleTimeoutBody::new(body, idle_timeout, label.clone());
        let body = if let Some(deadline) = grpc_response_deadline {
            GrpcDeadlineBody::new(
                body,
                deadline.deadline,
                deadline.timeout,
                label.clone(),
                deadline.timeout_message,
            )
            .boxed_unsync()
        } else {
            body.boxed_unsync()
        };
        let body = GrpcWebResponseBody::new(
            ActivePeerBody::new(
                body,
                active_peer.expect("non-upgrade responses should track peer activity"),
            ),
            fallback_trailers,
        );
        if grpc_web_mode.is_text() {
            GrpcWebTextEncodeBody::new(body).boxed_unsync()
        } else {
            body.boxed_unsync()
        }
    } else {
        let body = IdleTimeoutBody::new(body, idle_timeout, label.clone());
        let body = if let Some(deadline) = grpc_response_deadline {
            GrpcDeadlineBody::new(
                body,
                deadline.deadline,
                deadline.timeout,
                label,
                deadline.timeout_message,
            )
            .boxed_unsync()
        } else {
            body.boxed_unsync()
        };
        ActivePeerBody::new(
            body,
            active_peer.expect("non-upgrade responses should track peer activity"),
        )
        .boxed_unsync()
    };

    Response::from_parts(parts, body)
}
