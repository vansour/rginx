use super::grpc_web::{
    GrpcWebEncoding, GrpcWebMode, GrpcWebResponseBody, GrpcWebTextEncodeBody,
    extract_grpc_initial_trailers,
};
use super::health::{ActivePeerBody, ActivePeerGuard};
use super::request_body::{PrepareRequestError, PreparedProxyRequest, can_retry_peer_request};
use super::upgrade::proxy_upgraded_connection;
use super::*;
use std::error::Error as StdError;

#[derive(Debug, Clone, Copy)]
pub struct DownstreamRequestOptions {
    pub request_body_read_timeout: Option<Duration>,
    pub max_request_body_bytes: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
pub struct DownstreamRequestContext<'a> {
    pub listener_id: &'a str,
    pub downstream_proto: &'a str,
    pub request_id: &'a str,
    pub options: DownstreamRequestOptions,
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
    listener_id: &str,
    target: &ProxyTarget,
    client_address: ClientAddress,
    downstream: DownstreamRequestContext<'_>,
) -> HttpResponse {
    let request_headers = request.headers().clone();
    state.record_upstream_request(&target.upstream_name);
    let grpc_web_mode = match detect_grpc_web_mode(request.headers()) {
        Ok(mode) => mode,
        Err(message) => {
            state.record_upstream_unsupported_media_type_response(&target.upstream_name);
            return unsupported_media_type(&request_headers, format!("{message}\n"));
        }
    };
    let upstream_request_timeout =
        match effective_upstream_request_timeout(&request_headers, target.upstream.request_timeout)
        {
            Ok(timeout) => timeout,
            Err(message) => {
                state.record_upstream_bad_request_response(&target.upstream_name);
                return bad_request(&request_headers, format!("{message}\n"));
            }
        };
    let client = match clients.for_upstream(target.upstream.as_ref()) {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(
                request_id = %downstream.request_id,
                upstream = %target.upstream_name,
                upstream_sni_enabled = target.upstream.server_name,
                upstream_server_name = target.upstream.server_name_override.as_deref().unwrap_or("-"),
                upstream_verify = super::upstream_tls_verify_label(&target.upstream.tls),
                upstream_tls_failure = super::classify_upstream_tls_failure(&error),
                %error,
                "failed to select proxy client"
            );
            state.record_upstream_bad_gateway_response(&target.upstream_name);
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
        downstream.options.request_body_read_timeout,
        target.upstream.write_timeout,
        target.upstream.max_replayable_request_body_bytes,
        downstream.options.max_request_body_bytes,
        grpc_web_mode.as_ref(),
    )
    .await
    {
        Ok(request) => request,
        Err(PrepareRequestError::PayloadTooLarge { max_request_body_bytes }) => {
            tracing::info!(
                request_id = %downstream.request_id,
                upstream = %target.upstream_name,
                max_request_body_bytes,
                "rejecting request body that exceeds configured server limit"
            );
            state.record_upstream_payload_too_large_response(&target.upstream_name);
            return payload_too_large(
                &request_headers,
                format!(
                    "request body exceeds server.max_request_body_bytes ({max_request_body_bytes} bytes)\n"
                ),
            );
        }
        Err(error) => {
            tracing::warn!(
                request_id = %downstream.request_id,
                upstream = %target.upstream_name,
                %error,
                "failed to prepare upstream request"
            );
            if invalid_downstream_request_body_error(&error) {
                state.record_upstream_bad_request_response(&target.upstream_name);
                return bad_request(
                    &request_headers,
                    format!("invalid downstream request body: {error}\n"),
                );
            }
            state.record_upstream_bad_gateway_response(&target.upstream_name);
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
                        request_id = %downstream.request_id,
                        upstream = %target.upstream_name,
                        skipped_unhealthy = selected.skipped_unhealthy,
                        "proxy route has no healthy peers available"
        );
        state.record_upstream_no_healthy_peers(&target.upstream_name);
        state.record_upstream_bad_gateway_response(&target.upstream_name);
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
            downstream.downstream_proto,
            grpc_web_mode.as_ref(),
        ) {
            Ok(request) => request,
            Err(error) => {
                tracing::warn!(
                    request_id = %downstream.request_id,
                    upstream = %target.upstream_name,
                    peer = %peer.url,
                    upstream_sni_enabled = target.upstream.server_name,
                    upstream_server_name = target.upstream.server_name_override.as_deref().unwrap_or("-"),
                    upstream_verify = super::upstream_tls_verify_label(&target.upstream.tls),
                    %error,
                    "failed to build upstream request"
                );
                state.record_upstream_bad_gateway_response(&target.upstream_name);
                return bad_gateway(
                    &request_headers,
                    format!("failed to build upstream request for `{}`\n", target.upstream_name),
                );
            }
        };
        state.record_upstream_peer_attempt(&target.upstream_name, &peer.url);
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
                state.record_upstream_peer_success(&target.upstream_name, &peer.url);
                state.record_upstream_completed_response(&target.upstream_name);
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
                        request_id = %downstream.request_id,
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
                    let connection_guard = state.retain_connection_slot(listener_id);
                    state.spawn_background_task(proxy_upgraded_connection(
                        downstream_upgrade,
                        upstream_upgrade,
                        target.upstream_name.clone(),
                        peer.url.clone(),
                        active_peer,
                        connection_guard,
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
                state.record_upstream_peer_failure(&target.upstream_name, &peer.url);
                let tls_failure = super::classify_upstream_tls_failure(&error);
                state.record_upstream_peer_failure_class(&target.upstream_name, tls_failure);
                state.record_upstream_failover(&target.upstream_name);
                let failure = clients.record_peer_failure(&target.upstream_name, &peer.url);
                let next_peer = &peers[attempt_index + 1];
                tracing::warn!(
                    request_id = %downstream.request_id,
                    upstream = %target.upstream_name,
                    failed_peer = %peer.url,
                    next_peer = %next_peer.url,
                    attempt = attempt_index + 1,
                    upstream_sni_enabled = target.upstream.server_name,
                    upstream_server_name = target.upstream.server_name_override.as_deref().unwrap_or("-"),
                    upstream_verify = super::upstream_tls_verify_label(&target.upstream.tls),
                    upstream_tls_failure = super::classify_upstream_tls_failure(&error),
                    consecutive_failures = failure.consecutive_failures,
                    entered_cooldown = failure.entered_cooldown,
                    %error,
                    "retrying idempotent upstream request on alternate peer"
                );
            }
            Ok(Err(error)) => {
                if invalid_downstream_request_body_error(&error) {
                    tracing::warn!(
                        request_id = %downstream.request_id,
                        upstream = %target.upstream_name,
                        peer = %peer.url,
                        upstream_sni_enabled = target.upstream.server_name,
                        upstream_server_name = target.upstream.server_name_override.as_deref().unwrap_or("-"),
                        upstream_verify = super::upstream_tls_verify_label(&target.upstream.tls),
                        upstream_tls_failure = super::classify_upstream_tls_failure(&error),
                        %error,
                        "downstream request body was invalid while proxying upstream request"
                    );
                    state.record_upstream_bad_request_response(&target.upstream_name);
                    return bad_request(
                        &request_headers,
                        format!("invalid downstream request body: {error}\n"),
                    );
                }
                state.record_upstream_peer_failure(&target.upstream_name, &peer.url);
                let tls_failure = super::classify_upstream_tls_failure(&error);
                state.record_upstream_peer_failure_class(&target.upstream_name, tls_failure);
                let failure = clients.record_peer_failure(&target.upstream_name, &peer.url);
                tracing::warn!(
                    request_id = %downstream.request_id,
                    upstream = %target.upstream_name,
                    peer = %peer.url,
                    upstream_sni_enabled = target.upstream.server_name,
                    upstream_server_name = target.upstream.server_name_override.as_deref().unwrap_or("-"),
                    upstream_verify = super::upstream_tls_verify_label(&target.upstream.tls),
                    upstream_tls_failure = super::classify_upstream_tls_failure(&error),
                    consecutive_failures = failure.consecutive_failures,
                    entered_cooldown = failure.entered_cooldown,
                    %error,
                    "upstream request failed"
                );
                state.record_upstream_bad_gateway_response(&target.upstream_name);
                return bad_gateway(
                    &request_headers,
                    format!("upstream `{}` is unavailable\n", target.upstream_name),
                );
            }
            Err(error) if can_retry_peer_request(&prepared_request, &peers, attempt_index) => {
                state.record_upstream_peer_timeout(&target.upstream_name, &peer.url);
                state.record_upstream_failover(&target.upstream_name);
                let failure = clients.record_peer_failure(&target.upstream_name, &peer.url);
                let next_peer = &peers[attempt_index + 1];
                tracing::warn!(
                    request_id = %downstream.request_id,
                    upstream = %target.upstream_name,
                    failed_peer = %peer.url,
                    next_peer = %next_peer.url,
                    attempt = attempt_index + 1,
                    timeout_ms = upstream_request_timeout.as_millis() as u64,
                    upstream_sni_enabled = target.upstream.server_name,
                    upstream_server_name = target.upstream.server_name_override.as_deref().unwrap_or("-"),
                    upstream_verify = super::upstream_tls_verify_label(&target.upstream.tls),
                    upstream_tls_failure = super::classify_upstream_tls_failure(&error),
                    consecutive_failures = failure.consecutive_failures,
                    entered_cooldown = failure.entered_cooldown,
                    %error,
                    "retrying idempotent upstream request on alternate peer after timeout"
                );
            }
            Err(error) => {
                state.record_upstream_peer_timeout(&target.upstream_name, &peer.url);
                let failure = clients.record_peer_failure(&target.upstream_name, &peer.url);
                tracing::warn!(
                    request_id = %downstream.request_id,
                    upstream = %target.upstream_name,
                    peer = %peer.url,
                    timeout_ms = upstream_request_timeout.as_millis() as u64,
                    upstream_sni_enabled = target.upstream.server_name,
                    upstream_server_name = target.upstream.server_name_override.as_deref().unwrap_or("-"),
                    upstream_verify = super::upstream_tls_verify_label(&target.upstream.tls),
                    consecutive_failures = failure.consecutive_failures,
                    entered_cooldown = failure.entered_cooldown,
                    %error,
                    "upstream request timed out"
                );
                state.record_upstream_gateway_timeout_response(&target.upstream_name);
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

    state.record_upstream_bad_gateway_response(&target.upstream_name);
    bad_gateway(&request_headers, format!("upstream `{}` is unavailable\n", target.upstream_name))
}

fn invalid_downstream_request_body_error(error: &(dyn StdError + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(candidate) = current {
        if let Some(io_error) = candidate.downcast_ref::<std::io::Error>()
            && matches!(
                io_error.kind(),
                std::io::ErrorKind::InvalidData | std::io::ErrorKind::InvalidInput
            )
        {
            return true;
        }

        let message = candidate.to_string();
        if message.contains("grpc-web") && message.contains("invalid") {
            return true;
        }

        current = candidate.source();
    }

    false
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
