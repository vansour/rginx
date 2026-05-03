use super::*;

pub(super) trait ForwardCacheBackend {
    async fn lookup(
        &self,
        request: crate::cache::CacheRequest,
        downstream_scheme: &str,
        policy: &rginx_core::RouteCachePolicy,
    ) -> crate::cache::CacheLookup;

    fn record_bypass_for_zone(&self, zone_name: &str);

    async fn store_response(
        &self,
        context: crate::cache::CacheStoreContext,
        response: HttpResponse,
    ) -> HttpResponse;

    async fn complete_not_modified(
        &self,
        context: crate::cache::CacheStoreContext,
        response: HttpResponse,
    ) -> std::result::Result<HttpResponse, crate::cache::CacheStoreError>;
}

impl ForwardCacheBackend for crate::cache::CacheManager {
    async fn lookup(
        &self,
        request: crate::cache::CacheRequest,
        downstream_scheme: &str,
        policy: &rginx_core::RouteCachePolicy,
    ) -> crate::cache::CacheLookup {
        crate::cache::CacheManager::lookup(self, request, downstream_scheme, policy).await
    }

    fn record_bypass_for_zone(&self, zone_name: &str) {
        crate::cache::CacheManager::record_bypass_for_zone(self, zone_name);
    }

    async fn store_response(
        &self,
        context: crate::cache::CacheStoreContext,
        response: HttpResponse,
    ) -> HttpResponse {
        crate::cache::CacheManager::store_response(self, context, response).await
    }

    async fn complete_not_modified(
        &self,
        context: crate::cache::CacheStoreContext,
        response: HttpResponse,
    ) -> std::result::Result<HttpResponse, crate::cache::CacheStoreError> {
        crate::cache::CacheManager::complete_not_modified(self, context, response).await
    }
}

pub(super) struct ForwardCacheContext {
    pub(super) store: Option<Box<crate::cache::CacheStoreContext>>,
    pub(super) status: Option<crate::cache::CacheStatus>,
}

pub(super) enum ForwardCacheLookup {
    Hit(HttpResponse),
    Updating(HttpResponse, Box<ForwardCacheContext>),
    Proceed(Box<ForwardCacheContext>),
}

pub(super) async fn lookup_forward_cache<B: ForwardCacheBackend + ?Sized>(
    cache_backend: &B,
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
        cache_backend.record_bypass_for_zone(&policy.zone);
        return ForwardCacheLookup::Proceed(Box::new(ForwardCacheContext {
            store: None,
            status: Some(crate::cache::CacheStatus::Bypass),
        }));
    }

    match cache_backend.lookup(request, downstream_scheme, policy).await {
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
