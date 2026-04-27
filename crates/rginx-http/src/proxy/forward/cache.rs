use super::*;

pub(super) struct ForwardCacheContext {
    pub(super) store: Option<crate::cache::CacheStoreContext>,
    pub(super) status: Option<crate::cache::CacheStatus>,
}

pub(super) enum ForwardCacheLookup {
    Hit(HttpResponse),
    Proceed(ForwardCacheContext),
}

pub(super) async fn lookup_forward_cache(
    cache_manager: &crate::cache::CacheManager,
    request: crate::cache::CacheRequest,
    downstream_scheme: &str,
    policy: Option<&rginx_core::RouteCachePolicy>,
) -> ForwardCacheLookup {
    let Some(policy) = policy else {
        return ForwardCacheLookup::Proceed(ForwardCacheContext { store: None, status: None });
    };

    match cache_manager.lookup(request, downstream_scheme, policy).await {
        crate::cache::CacheLookup::Hit(response) => ForwardCacheLookup::Hit(response),
        crate::cache::CacheLookup::Miss(context) => {
            let status = context.cache_status();
            ForwardCacheLookup::Proceed(ForwardCacheContext {
                store: Some(context),
                status: Some(status),
            })
        }
        crate::cache::CacheLookup::Bypass(status) => {
            ForwardCacheLookup::Proceed(ForwardCacheContext { store: None, status: Some(status) })
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
}
