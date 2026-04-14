use super::request_body::{PrepareRequestError, PreparedProxyRequest, can_retry_peer_request};
use super::upgrade::proxy_upgraded_connection;
use super::*;

mod error;
mod grpc;
mod response;

use error::{
    bad_gateway, bad_request, downstream_request_body_too_large_error, gateway_timeout,
    grpc_timeout_message, invalid_downstream_request_body_error, payload_too_large,
    unsupported_media_type,
};
use grpc::grpc_protocol_request;
use response::{GrpcResponseDeadline, build_downstream_response};

pub(super) use error::wait_for_upstream_stage;
#[cfg(test)]
pub(super) use grpc::parse_grpc_timeout;
pub(super) use grpc::{detect_grpc_web_mode, effective_upstream_request_timeout};

#[derive(Debug, Clone, Copy)]
pub struct DownstreamRequestOptions {
    pub request_body_read_timeout: Option<Duration>,
    pub max_request_body_bytes: Option<usize>,
    pub request_buffering: RouteBufferingPolicy,
    pub streaming_response_idle_timeout: Option<Duration>,
}

#[derive(Debug, Clone, Copy)]
pub struct DownstreamRequestContext<'a> {
    pub listener_id: &'a str,
    pub downstream_proto: &'a str,
    pub request_id: &'a str,
    pub options: DownstreamRequestOptions,
}

pub async fn forward_request(
    state: SharedState,
    clients: ProxyClients,
    mut request: Request<HttpBody>,
    listener_id: &str,
    target: &ProxyTarget,
    client_address: ClientAddress,
    downstream: DownstreamRequestContext<'_>,
) -> HttpResponse {
    let request_headers = request.headers().clone();
    let response_idle_timeout =
        downstream.options.streaming_response_idle_timeout.unwrap_or(target.upstream.idle_timeout);
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
    if downstream_upgrade.is_some() && target.upstream.protocol == UpstreamProtocol::Http3 {
        state.record_upstream_bad_gateway_response(&target.upstream_name);
        return bad_gateway(
            &request_headers,
            format!("upstream `{}` does not support upgrade over http3\n", target.upstream_name),
        );
    }

    let mut prepared_request = match PreparedProxyRequest::from_request(
        request,
        &target.upstream_name,
        downstream.options.request_body_read_timeout,
        target.upstream.write_timeout,
        target.upstream.max_replayable_request_body_bytes,
        downstream.options.max_request_body_bytes,
        downstream.options.request_buffering,
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
            client.request(target.upstream.as_ref(), peer, upstream_request),
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
                        response_idle_timeout,
                        grpc_response_deadline.clone(),
                        grpc_web_mode.as_ref(),
                        None,
                    );
                }

                return build_downstream_response(
                    response,
                    &target.upstream_name,
                    &peer.url,
                    response_idle_timeout,
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
                if downstream_request_body_too_large_error(&error) {
                    let max_request_body_bytes =
                        downstream.options.max_request_body_bytes.unwrap_or_default();
                    tracing::info!(
                        request_id = %downstream.request_id,
                        upstream = %target.upstream_name,
                        peer = %peer.url,
                        max_request_body_bytes,
                        %error,
                        "rejecting streamed request body that exceeds configured server limit"
                    );
                    state.record_upstream_payload_too_large_response(&target.upstream_name);
                    return payload_too_large(
                        &request_headers,
                        format!(
                            "request body exceeds server.max_request_body_bytes ({max_request_body_bytes} bytes)\n"
                        ),
                    );
                }
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
