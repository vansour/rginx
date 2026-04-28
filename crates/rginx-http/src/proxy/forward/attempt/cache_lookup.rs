use super::*;

pub(super) async fn resolve_forward_cache(
    state: &SharedState,
    target: &ProxyTarget,
    client_address: &ClientAddress,
    downstream: &DownstreamRequestContext<'_>,
    cache_manager: &crate::cache::CacheManager,
    cache_request: crate::cache::CacheRequest,
) -> std::result::Result<ForwardCacheContext, HttpResponse> {
    match lookup_forward_cache(
        cache_manager,
        cache_request,
        downstream.downstream_proto,
        downstream.options.response_buffering,
        downstream.options.cache.as_ref(),
    )
    .await
    {
        ForwardCacheLookup::Hit(response) => Err(response),
        ForwardCacheLookup::Updating(response, mut cache) => {
            if let Some(store) = cache.store.take() {
                spawn_background_cache_refresh(state, target, client_address, downstream, *store);
            }
            Err(response)
        }
        ForwardCacheLookup::Proceed(cache) => Ok(*cache),
    }
}
