use super::*;

pub(super) struct ForwardCacheContext {
    pub(super) store: Option<Box<crate::cache::CacheStoreContext>>,
    pub(super) status: Option<crate::cache::CacheStatus>,
}

pub(super) enum ForwardCacheLookup {
    Hit(HttpResponse),
    Updating(HttpResponse, Box<ForwardCacheContext>),
    Proceed(Box<ForwardCacheContext>),
}

pub(super) async fn lookup_forward_cache(
    cache_manager: &crate::cache::CacheManager,
    request: crate::cache::CacheRequest,
    downstream_scheme: &str,
    response_buffering: rginx_core::RouteBufferingPolicy,
    policy: Option<&rginx_core::RouteCachePolicy>,
) -> ForwardCacheLookup {
    let Some(policy) = policy else {
        return ForwardCacheLookup::Proceed(Box::new(ForwardCacheContext {
            store: None,
            status: None,
        }));
    };

    if response_buffering == rginx_core::RouteBufferingPolicy::Off {
        cache_manager.record_bypass_for_zone(&policy.zone);
        return ForwardCacheLookup::Proceed(Box::new(ForwardCacheContext {
            store: None,
            status: Some(crate::cache::CacheStatus::Bypass),
        }));
    }

    match cache_manager.lookup(request, downstream_scheme, policy).await {
        crate::cache::CacheLookup::Hit(response) => ForwardCacheLookup::Hit(response),
        crate::cache::CacheLookup::Updating(response, context) => {
            let status = context.cache_status();
            ForwardCacheLookup::Updating(
                response,
                Box::new(ForwardCacheContext { store: Some(context), status: Some(status) }),
            )
        }
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

    pub(super) fn apply_upstream_request_method(&self, request: &mut http::Request<HttpBody>) {
        if let Some(store) =
            self.store.as_ref().filter(|store| store.prepares_cacheable_upstream_request())
        {
            *request.method_mut() = store.upstream_request_method();
        }
    }

    pub(super) fn apply_upstream_request_headers(&self, headers: &mut HeaderMap) {
        if let Some(store) =
            self.store.as_ref().filter(|store| store.prepares_cacheable_upstream_request())
        {
            store.apply_upstream_request_headers(headers);
        }
    }

    pub(super) fn apply_conditional_request_headers(&self, headers: &mut HeaderMap) {
        if let Some(store) =
            self.store.as_ref().filter(|store| store.prepares_cacheable_upstream_request())
        {
            store.apply_conditional_request_headers(headers);
        }
    }

    pub(super) async fn serve_stale_for_reason(
        &self,
        reason: crate::cache::CacheStaleReason,
        status: crate::cache::CacheStatus,
    ) -> Option<HttpResponse> {
        let store = self.store.as_ref()?;
        if !store.can_serve_stale(reason) {
            return None;
        }
        store.serve_stale(status).await
    }
}
