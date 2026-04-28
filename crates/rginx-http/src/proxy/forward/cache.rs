use super::*;

pub(super) struct ForwardCacheContext {
    pub(super) store: Option<Box<crate::cache::CacheStoreContext>>,
    pub(super) status: Option<crate::cache::CacheStatus>,
}

pub(super) enum ForwardCacheLookup {
    Hit(HttpResponse),
    Proceed(Box<ForwardCacheContext>),
}

pub(super) async fn lookup_forward_cache(
    cache_manager: &crate::cache::CacheManager,
    request: crate::cache::CacheRequest,
    downstream_scheme: &str,
    policy: Option<&rginx_core::RouteCachePolicy>,
) -> ForwardCacheLookup {
    let Some(policy) = policy else {
        return ForwardCacheLookup::Proceed(Box::new(ForwardCacheContext {
            store: None,
            status: None,
        }));
    };

    match cache_manager.lookup(request, downstream_scheme, policy).await {
        crate::cache::CacheLookup::Hit(response) => ForwardCacheLookup::Hit(response),
        crate::cache::CacheLookup::Miss(context) => {
            let status = context.cache_status();
            ForwardCacheLookup::Proceed(Box::new(ForwardCacheContext {
                store: Some(context),
                status: Some(status),
            }))
        }
        crate::cache::CacheLookup::Bypass(status) => {
            ForwardCacheLookup::Proceed(Box::new(ForwardCacheContext {
                store: None,
                status: Some(status),
            }))
        }
    }
}

impl ForwardCacheContext {
    pub(super) fn mark_response(&self, response: HttpResponse) -> HttpResponse {
        if let Some(status) = self.status {
            crate::cache::with_cache_status(response, status)
        } else {
            response
        }
    }

    pub(super) fn apply_conditional_request_headers(&self, headers: &mut HeaderMap) {
        if let Some(store) = self.store.as_ref() {
            store.apply_conditional_request_headers(headers);
        }
    }
    pub(super) async fn serve_stale_on_error(&self) -> Option<HttpResponse> {
        let store = self.store.as_ref()?;
        if !store.can_serve_stale_on_error() {
            return None;
        }
        store.serve_stale_on_error().await
    }
}
