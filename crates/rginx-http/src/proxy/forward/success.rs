use super::*;

pub(super) struct UpstreamSuccessContext<'a> {
    pub(super) state: &'a SharedState,
    pub(super) downstream_upgrade: Option<OnUpgrade>,
    pub(super) listener_id: &'a str,
    pub(super) target: &'a ProxyTarget,
    pub(super) peer: &'a ResolvedUpstreamPeer,
    pub(super) active_peer: super::super::health::ActivePeerGuard,
    pub(super) response_idle_timeout: Duration,
    pub(super) grpc_response_deadline: Option<super::response::GrpcResponseDeadline>,
    pub(super) grpc_web_mode: Option<&'a GrpcWebMode>,
    pub(super) cache_manager: crate::cache::CacheManager,
    pub(super) cache_store: Option<crate::cache::CacheStoreContext>,
    pub(super) cache_status: Option<crate::cache::CacheStatus>,
}

pub(super) async fn finalize_upstream_success(
    response: Response<HttpBody>,
    context: UpstreamSuccessContext<'_>,
) -> HttpResponse {
    if context
        .cache_store
        .as_ref()
        .is_some_and(|cache_store| cache_store.should_refresh_from_not_modified(response.status()))
    {
        return match context
            .cache_manager
            .complete_not_modified(
                context.cache_store.expect("cache store should be present for revalidation"),
                response,
            )
            .await
        {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(%error, "failed to refresh cached metadata from 304 response");
                crate::handler::text_response(
                    StatusCode::BAD_GATEWAY,
                    "text/plain; charset=utf-8",
                    format!("failed to refresh cached response from upstream 304: {error}\n"),
                )
            }
        };
    }

    if let Some(cache_store) = context.cache_store.as_ref() {
        if response.status().is_server_error()
            && let Some(stale) = cache_store.serve_stale_on_error().await
        {
            return stale;
        }
    }

    let mut response = response;
    let upstream_upgrade = if context.downstream_upgrade.is_some()
        && is_upgrade_response(response.status(), response.headers())
    {
        Some(hyper::upgrade::on(&mut response))
    } else {
        None
    };

    if let (Some(downstream_upgrade), Some(upstream_upgrade)) =
        (context.downstream_upgrade, upstream_upgrade)
    {
        let connection_guard = context.state.retain_connection_slot(context.listener_id);
        context.state.spawn_background_task(proxy_upgraded_connection(
            downstream_upgrade,
            upstream_upgrade,
            context.target.upstream_name.clone(),
            context.peer.display_url.clone(),
            context.active_peer,
            connection_guard,
        ));
        let response = build_downstream_response(
            response,
            &context.target.upstream_name,
            &context.peer.display_url,
            context.response_idle_timeout,
            context.grpc_response_deadline,
            context.grpc_web_mode,
            None,
        );
        return apply_cache_store(&context.cache_manager, None, context.cache_status, response)
            .await;
    }

    let response = build_downstream_response(
        response,
        &context.target.upstream_name,
        &context.peer.display_url,
        context.response_idle_timeout,
        context.grpc_response_deadline,
        context.grpc_web_mode,
        Some(context.active_peer),
    );
    apply_cache_store(&context.cache_manager, context.cache_store, context.cache_status, response)
        .await
}

async fn apply_cache_store(
    cache_manager: &crate::cache::CacheManager,
    cache_store: Option<crate::cache::CacheStoreContext>,
    cache_status: Option<crate::cache::CacheStatus>,
    response: HttpResponse,
) -> HttpResponse {
    if let Some(cache_store) = cache_store {
        cache_manager.store_response(cache_store, response).await
    } else if let Some(cache_status) = cache_status {
        crate::cache::with_cache_status(response, cache_status)
    } else {
        response
    }
}
