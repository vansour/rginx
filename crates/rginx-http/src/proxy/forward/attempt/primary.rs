use super::*;

pub async fn forward_request(
    state: SharedState,
    active: crate::state::ActiveState,
    request: Request<HttpBody>,
    listener_id: &str,
    target: &ProxyTarget,
    client_address: ClientAddress,
    downstream: DownstreamRequestContext<'_>,
) -> HttpResponse {
    let mut request = request;
    let cache_manager = active.cache.clone();
    let clients = active.clients;
    let cache_request = crate::cache::CacheRequest::from_request(&request);
    let mut cache = match resolve_forward_cache(
        &state,
        target,
        &client_address,
        &downstream,
        &cache_manager,
        cache_request,
    )
    .await
    {
        Ok(cache) => cache,
        Err(response) => return response,
    };
    cache.apply_upstream_request_method(&mut request);
    cache.apply_upstream_request_headers(request.headers_mut());
    cache.apply_conditional_request_headers(request.headers_mut());

    let prepared = match prepare_forward_request(
        &state,
        &clients,
        request,
        target,
        &client_address,
        &downstream,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(response) => return cache.mark_response(response),
    };

    let setup::PreparedForwardRequest {
        request_headers,
        response_idle_timeout,
        grpc_web_mode,
        upstream_request_timeout,
        client,
        downstream_upgrade,
        mut prepared_request,
        peers,
    } = prepared;

    for (attempt_index, peer) in peers.iter().enumerate() {
        let grpc_response_deadline = grpc_response_deadline(
            &request_headers,
            &target.upstream_name,
            upstream_request_timeout,
        );
        let built_request = match prepared_request.build_for_peer(
            peer,
            target,
            &client_address,
            downstream.downstream_proto,
            grpc_web_mode.as_ref(),
        ) {
            Ok(request) => request,
            Err(error) => {
                return failed_to_build_request_response(
                    &state,
                    &request_headers,
                    target,
                    peer,
                    &downstream,
                    error,
                    &cache,
                );
            }
        };
        let body_completion = built_request.body_completion;
        let upstream_request = built_request.request;
        state.record_upstream_peer_attempt(&target.upstream_name, &peer.logical_peer_url);
        let active_peer = clients.track_active_request(&target.upstream_name, &peer.endpoint_key);
        let upstream_started = std::time::Instant::now();

        match wait_for_upstream_stage(
            upstream_request_timeout,
            &target.upstream_name,
            "request",
            client.request(target.upstream.as_ref(), peer, upstream_request),
        )
        .await
        {
            Ok(Ok(response)) => {
                if let Err(response) = finalize_streaming_request_body(
                    body_completion,
                    &state,
                    &request_headers,
                    target,
                    peer,
                    &downstream,
                )
                .await
                {
                    return cache.mark_response(response);
                }
                state.record_upstream_peer_success(&target.upstream_name, &peer.logical_peer_url);
                state.record_upstream_completed_response(&target.upstream_name);
                let recovered =
                    clients.record_peer_success(&target.upstream_name, &peer.endpoint_key);
                log_successful_attempt(target, peer, &downstream, attempt_index, recovered);

                return finalize_upstream_success(
                    response,
                    UpstreamSuccessContext {
                        state: &state,
                        downstream_upgrade: downstream_upgrade.clone(),
                        listener_id,
                        target,
                        peer,
                        active_peer,
                        response_idle_timeout,
                        grpc_response_deadline,
                        grpc_web_mode: grpc_web_mode.as_ref(),
                        cache_backend: &cache_manager,
                        cache_store: cache.store.take(),
                        cache_status: cache.status,
                        upstream_response_time_ms: upstream_started.elapsed().as_millis() as u64,
                    },
                )
                .await;
            }
            Ok(Err(error))
                if can_retry_peer_request(&prepared_request, peers.len(), attempt_index) =>
            {
                state.record_upstream_peer_failure(&target.upstream_name, &peer.logical_peer_url);
                let tls_failure = super::classify_upstream_tls_failure(&error);
                state.record_upstream_peer_failure_class(&target.upstream_name, tls_failure);
                state.record_upstream_failover(&target.upstream_name);
                let failure =
                    clients.record_peer_failure(&target.upstream_name, &peer.endpoint_key);
                let next_peer = &peers[attempt_index + 1];
                tracing::warn!(
                    request_id = %downstream.request_id,
                    upstream = %target.upstream_name,
                    failed_peer = %peer.display_url,
                    failed_logical_peer = %peer.logical_peer_url,
                    next_peer = %next_peer.display_url,
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
                if let Some(max_request_body_bytes) = downstream_request_body_limit(&error) {
                    tracing::info!(
                        request_id = %downstream.request_id,
                        upstream = %target.upstream_name,
                        peer = %peer.display_url,
                        logical_peer = %peer.logical_peer_url,
                        max_request_body_bytes,
                        %error,
                        "rejecting streamed request body that exceeds configured server limit"
                    );
                    state.record_upstream_payload_too_large_response(&target.upstream_name);
                    return cache.mark_response(payload_too_large(
                        &request_headers,
                        format!(
                            "request body exceeds server.max_request_body_bytes ({max_request_body_bytes} bytes)\n"
                        ),
                    ));
                }
                if invalid_downstream_request_body_error(&error) {
                    tracing::warn!(
                        request_id = %downstream.request_id,
                        upstream = %target.upstream_name,
                        peer = %peer.display_url,
                        logical_peer = %peer.logical_peer_url,
                        upstream_sni_enabled = target.upstream.server_name,
                        upstream_server_name = target.upstream.server_name_override.as_deref().unwrap_or("-"),
                        upstream_verify = super::upstream_tls_verify_label(&target.upstream.tls),
                        upstream_tls_failure = super::classify_upstream_tls_failure(&error),
                        %error,
                        "downstream request body was invalid while proxying upstream request"
                    );
                    state.record_upstream_bad_request_response(&target.upstream_name);
                    return cache.mark_response(bad_request(
                        &request_headers,
                        format!("invalid downstream request body: {error}\n"),
                    ));
                }
                state.record_upstream_peer_failure(&target.upstream_name, &peer.logical_peer_url);
                let tls_failure = super::classify_upstream_tls_failure(&error);
                state.record_upstream_peer_failure_class(&target.upstream_name, tls_failure);
                let failure =
                    clients.record_peer_failure(&target.upstream_name, &peer.endpoint_key);
                tracing::warn!(
                    request_id = %downstream.request_id,
                    upstream = %target.upstream_name,
                    peer = %peer.display_url,
                    logical_peer = %peer.logical_peer_url,
                    upstream_sni_enabled = target.upstream.server_name,
                    upstream_server_name = target.upstream.server_name_override.as_deref().unwrap_or("-"),
                    upstream_verify = super::upstream_tls_verify_label(&target.upstream.tls),
                    upstream_tls_failure = super::classify_upstream_tls_failure(&error),
                    consecutive_failures = failure.consecutive_failures,
                    entered_cooldown = failure.entered_cooldown,
                    %error,
                    "upstream request failed"
                );
                if let Some(stale) = cache
                    .serve_stale_for_reason(
                        crate::cache::CacheStaleReason::Error,
                        crate::cache::CacheStatus::Stale,
                    )
                    .await
                {
                    return stale;
                }
                state.record_upstream_bad_gateway_response(&target.upstream_name);
                return cache.mark_response(bad_gateway(
                    &request_headers,
                    format!("upstream `{}` is unavailable\n", target.upstream_name),
                ));
            }
            Err(error) if can_retry_peer_request(&prepared_request, peers.len(), attempt_index) => {
                state.record_upstream_peer_timeout(&target.upstream_name, &peer.logical_peer_url);
                state.record_upstream_failover(&target.upstream_name);
                let failure =
                    clients.record_peer_failure(&target.upstream_name, &peer.endpoint_key);
                let next_peer = &peers[attempt_index + 1];
                tracing::warn!(
                    request_id = %downstream.request_id,
                    upstream = %target.upstream_name,
                    failed_peer = %peer.display_url,
                    failed_logical_peer = %peer.logical_peer_url,
                    next_peer = %next_peer.display_url,
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
                state.record_upstream_peer_timeout(&target.upstream_name, &peer.logical_peer_url);
                let failure =
                    clients.record_peer_failure(&target.upstream_name, &peer.endpoint_key);
                tracing::warn!(
                    request_id = %downstream.request_id,
                    upstream = %target.upstream_name,
                    peer = %peer.display_url,
                    logical_peer = %peer.logical_peer_url,
                    timeout_ms = upstream_request_timeout.as_millis() as u64,
                    upstream_sni_enabled = target.upstream.server_name,
                    upstream_server_name = target.upstream.server_name_override.as_deref().unwrap_or("-"),
                    upstream_verify = super::upstream_tls_verify_label(&target.upstream.tls),
                    consecutive_failures = failure.consecutive_failures,
                    entered_cooldown = failure.entered_cooldown,
                    %error,
                    "upstream request timed out"
                );
                if let Some(stale) = cache
                    .serve_stale_for_reason(
                        crate::cache::CacheStaleReason::Timeout,
                        crate::cache::CacheStatus::Stale,
                    )
                    .await
                {
                    return stale;
                }
                state.record_upstream_gateway_timeout_response(&target.upstream_name);
                return cache.mark_response(gateway_timeout(
                    &request_headers,
                    format!(
                        "{}\n",
                        grpc_timeout_message(&target.upstream_name, upstream_request_timeout)
                    ),
                ));
            }
        }
    }

    upstream_unavailable_response(&state, &request_headers, target, &cache)
}
