use super::*;

impl SharedState {
    pub(crate) async fn lookup_cached_response(
        &self,
        request: crate::cache::CacheRequest,
        downstream_scheme: &str,
        policy: &rginx_core::RouteCachePolicy,
    ) -> CacheLookup {
        let cache = { self.cache.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone() };
        cache.lookup(request, downstream_scheme, policy).await
    }

    pub(crate) async fn store_cached_response(
        &self,
        context: CacheStoreContext,
        response: crate::handler::HttpResponse,
    ) -> crate::handler::HttpResponse {
        let cache = { self.cache.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone() };
        cache.store_response(context, response).await
    }

    pub(crate) fn mark_cache_status(
        &self,
        response: crate::handler::HttpResponse,
        status: CacheStatus,
    ) -> crate::handler::HttpResponse {
        crate::cache::with_cache_status(response, status)
    }
}
