use super::*;

struct BackgroundCacheRefreshTask<B> {
    state: SharedState,
    target: ProxyTarget,
    client_address: ClientAddress,
    listener_id: String,
    downstream_proto: String,
    request_id: String,
    options: DownstreamRequestOptions,
    cache_backend: B,
    cache_store: crate::cache::CacheStoreContext,
}

pub(super) fn spawn_background_cache_refresh<B>(
    state: &SharedState,
    target: &ProxyTarget,
    client_address: &ClientAddress,
    downstream: &DownstreamRequestContext<'_>,
    cache_backend: B,
    cache_store: crate::cache::CacheStoreContext,
) where
    B: ForwardCacheBackend + Send + Sync + 'static,
{
    let task = BackgroundCacheRefreshTask {
        state: state.clone(),
        target: target.clone(),
        client_address: client_address.clone(),
        listener_id: downstream.listener_id.to_string(),
        downstream_proto: downstream.downstream_proto.to_string(),
        request_id: format!("{}:cache-update", downstream.request_id),
        options: downstream.options.clone(),
        cache_backend,
        cache_store,
    };
    state.clone().spawn_background_task(async move {
        run_background_cache_refresh(task).await;
    });
}

async fn run_background_cache_refresh<B>(task: BackgroundCacheRefreshTask<B>)
where
    B: ForwardCacheBackend + Send + Sync + 'static,
{
    let active = task.state.snapshot().await;
    let clients = active.clients;
    run_background_cache_refresh_with_backend(task, clients).await;
}

async fn run_background_cache_refresh_with_backend<B>(
    task: BackgroundCacheRefreshTask<B>,
    clients: ProxyClients,
) where
    B: ForwardCacheBackend + Send + Sync + 'static,
{
    let BackgroundCacheRefreshTask {
        state,
        target,
        client_address,
        listener_id,
        downstream_proto,
        request_id,
        options,
        cache_backend,
        cache_store,
    } = task;
    let mut request = cache_store.build_background_request();
    cache_store.apply_conditional_request_headers(request.headers_mut());

    let downstream = DownstreamRequestContext {
        listener_id: &listener_id,
        downstream_proto: &downstream_proto,
        request_id: &request_id,
        options,
    };

    let prepared = match prepare_forward_request(
        &state,
        &clients,
        request,
        &target,
        &client_address,
        &downstream,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(_) => return,
    };

    let setup::PreparedForwardRequest {
        request_headers,
        response_idle_timeout,
        grpc_web_mode,
        upstream_request_timeout,
        client,
        downstream_upgrade: _,
        mut prepared_request,
        peers,
    } = prepared;

    let mut cache_store = Some(cache_store);

    for (attempt_index, peer) in peers.iter().enumerate() {
        let grpc_response_deadline = grpc_response_deadline(
            &request_headers,
            &target.upstream_name,
            upstream_request_timeout,
        );
        let built_request = match prepared_request.build_for_peer(
            peer,
            &target,
            &client_address,
            downstream.downstream_proto,
            grpc_web_mode.as_ref(),
        ) {
            Ok(request) => request,
            Err(error) => {
                tracing::warn!(
                    request_id = %downstream.request_id,
                    upstream = %target.upstream_name,
                    peer = %peer.display_url,
                    logical_peer = %peer.logical_peer_url,
                    %error,
                    "failed to build background cache refresh request"
                );
                return;
            }
        };
        let body_completion = built_request.body_completion;
        let upstream_request = built_request.request;
        state.record_upstream_peer_attempt(&target.upstream_name, &peer.logical_peer_url);
        let active_peer = clients.track_active_request(&target.upstream_name, &peer.endpoint_key);

        match wait_for_upstream_stage(
            upstream_request_timeout,
            &target.upstream_name,
            "request",
            client.request(target.upstream.as_ref(), peer, upstream_request),
        )
        .await
        {
            Ok(Ok(response)) => {
                if finalize_streaming_request_body(
                    body_completion,
                    &state,
                    &request_headers,
                    &target,
                    peer,
                    &downstream,
                )
                .await
                .is_err()
                {
                    return;
                }

                state.record_upstream_peer_success(&target.upstream_name, &peer.logical_peer_url);
                state.record_upstream_completed_response(&target.upstream_name);
                let _ = clients.record_peer_success(&target.upstream_name, &peer.endpoint_key);

                let cache_store = cache_store
                    .take()
                    .expect("background cache refresh should keep a cache store context");
                if cache_store.should_refresh_from_not_modified(response.status()) {
                    if let Err(error) =
                        cache_backend.complete_not_modified(cache_store, response).await
                    {
                        tracing::warn!(
                            %error,
                            "failed to refresh cached metadata from background 304 response"
                        );
                    }
                    return;
                }

                let response = build_downstream_response(
                    response,
                    &target.upstream_name,
                    &peer.display_url,
                    response_idle_timeout,
                    grpc_response_deadline,
                    grpc_web_mode.as_ref(),
                    Some(active_peer),
                );
                drain_background_cache_refresh_response(
                    cache_backend.store_response(cache_store, response).await,
                    &target.upstream_name,
                    &peer.display_url,
                    &downstream,
                )
                .await;
                return;
            }
            Ok(Err(error))
                if can_retry_peer_request(&prepared_request, peers.len(), attempt_index) =>
            {
                state.record_upstream_peer_failure(&target.upstream_name, &peer.logical_peer_url);
                let tls_failure = super::classify_upstream_tls_failure(&error);
                state.record_upstream_peer_failure_class(&target.upstream_name, tls_failure);
                state.record_upstream_failover(&target.upstream_name);
                let _ = clients.record_peer_failure(&target.upstream_name, &peer.endpoint_key);
            }
            Ok(Err(error)) => {
                state.record_upstream_peer_failure(&target.upstream_name, &peer.logical_peer_url);
                let tls_failure = super::classify_upstream_tls_failure(&error);
                state.record_upstream_peer_failure_class(&target.upstream_name, tls_failure);
                let _ = clients.record_peer_failure(&target.upstream_name, &peer.endpoint_key);
                tracing::warn!(
                    request_id = %downstream.request_id,
                    upstream = %target.upstream_name,
                    peer = %peer.display_url,
                    logical_peer = %peer.logical_peer_url,
                    %error,
                    "background cache refresh request failed"
                );
                return;
            }
            Err(_error)
                if can_retry_peer_request(&prepared_request, peers.len(), attempt_index) =>
            {
                state.record_upstream_peer_timeout(&target.upstream_name, &peer.logical_peer_url);
                state.record_upstream_failover(&target.upstream_name);
                let _ = clients.record_peer_failure(&target.upstream_name, &peer.endpoint_key);
            }
            Err(error) => {
                state.record_upstream_peer_timeout(&target.upstream_name, &peer.logical_peer_url);
                let _ = clients.record_peer_failure(&target.upstream_name, &peer.endpoint_key);
                tracing::warn!(
                    request_id = %downstream.request_id,
                    upstream = %target.upstream_name,
                    peer = %peer.display_url,
                    logical_peer = %peer.logical_peer_url,
                    timeout_ms = upstream_request_timeout.as_millis() as u64,
                    %error,
                    "background cache refresh request timed out"
                );
                return;
            }
        }
    }
}

async fn drain_background_cache_refresh_response(
    response: HttpResponse,
    upstream_name: &str,
    peer_url: &str,
    downstream: &DownstreamRequestContext<'_>,
) {
    let mut body = response.into_body();
    while let Some(frame) = body.frame().await {
        if let Err(error) = frame {
            tracing::warn!(
                request_id = %downstream.request_id,
                upstream = %upstream_name,
                peer = %peer_url,
                %error,
                "background cache refresh response body failed while draining"
            );
            return;
        }
    }
}
