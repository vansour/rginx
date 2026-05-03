use super::*;

pub(super) async fn resolve_forward_cache<B>(
    state: &SharedState,
    target: &ProxyTarget,
    client_address: &ClientAddress,
    downstream: &DownstreamRequestContext<'_>,
    cache_backend: &B,
    cache_request: crate::cache::CacheRequest,
) -> std::result::Result<ForwardCacheContext, HttpResponse>
where
    B: ForwardCacheBackend + Clone + Send + Sync + 'static,
{
    match lookup_forward_cache(
        cache_backend,
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
                spawn_background_cache_refresh(
                    state,
                    target,
                    client_address,
                    downstream,
                    cache_backend.clone(),
                    *store,
                );
            }
            Err(response)
        }
        ForwardCacheLookup::Proceed(cache) => Ok(*cache),
    }
}
