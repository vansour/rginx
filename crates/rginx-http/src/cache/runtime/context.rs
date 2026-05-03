use super::support::{stale_if_error_window_open, use_stale_matches_status};
use super::*;

impl CacheRequest {
    pub(crate) fn from_request(request: &Request<HttpBody>) -> Self {
        Self {
            method: request.method().clone(),
            uri: request.uri().clone(),
            headers: request.headers().clone(),
        }
    }

    pub(crate) fn with_method(&self, method: Method) -> Self {
        Self { method, uri: self.uri.clone(), headers: self.headers.clone() }
    }

    pub(crate) fn request_uri(&self) -> &str {
        self.uri.path_and_query().map(|value| value.as_str()).unwrap_or("/")
    }
}

impl CacheStoreContext {
    pub(crate) fn cache_status(&self) -> CacheStatus {
        self.cache_status
    }

    pub(crate) fn prepares_cacheable_upstream_request(&self) -> bool {
        self.store_response
    }

    pub(crate) fn upstream_request_method(&self) -> Method {
        request::upstream_cache_request_method(&self.request.method, &self.policy)
    }

    pub(crate) fn build_background_request(&self) -> Request<HttpBody> {
        let method = request::upstream_cache_request_method(&self.request.method, &self.policy);
        let mut request = Request::builder()
            .method(method)
            .uri(self.request.uri.clone())
            .body(crate::handler::full_body(bytes::Bytes::new()))
            .expect("background cache refresh request should build");
        *request.headers_mut() = self.request.headers.clone();
        self.apply_upstream_request_headers(request.headers_mut());
        request
    }

    pub(crate) fn apply_upstream_request_headers(&self, headers: &mut HeaderMap) {
        request::apply_upstream_range_headers(&self.request.method, headers, &self.policy);
    }

    pub(crate) fn apply_conditional_request_headers(&self, headers: &mut HeaderMap) {
        let Some(conditional_headers) =
            self.cached_response_head.as_ref().and_then(|head| head.conditional_headers.as_ref())
        else {
            return;
        };
        if let Some(value) = conditional_headers.if_none_match.clone() {
            headers.insert(IF_NONE_MATCH, value);
        }
        if let Some(value) = conditional_headers.if_modified_since.clone() {
            headers.insert(IF_MODIFIED_SINCE, value);
        }
    }

    pub(crate) fn should_refresh_from_not_modified(&self, status: StatusCode) -> bool {
        self.cached_entry.is_some() && status == StatusCode::NOT_MODIFIED
    }

    pub(crate) fn can_serve_stale(&self, reason: CacheStaleReason) -> bool {
        let Some(entry) = &self.cached_entry else {
            return false;
        };
        if self.cached_response_head.is_none() {
            return false;
        }
        if entry.is_hit_for_pass() {
            return false;
        }
        if self.request_forces_revalidation || entry.requires_revalidation || entry.must_revalidate
        {
            return false;
        }

        let now = unix_time_ms(SystemTime::now());
        let phase = lifecycle_phase(entry, now);
        if phase == CacheEntryLifecyclePhase::Dead {
            return false;
        }
        match reason {
            CacheStaleReason::Error => {
                (phase == CacheEntryLifecyclePhase::Grace
                    && self.policy.use_stale.contains(&rginx_core::CacheUseStaleCondition::Error))
                    || stale_if_error_window_open(entry, now)
            }
            CacheStaleReason::Timeout => {
                (phase == CacheEntryLifecyclePhase::Grace
                    && self.policy.use_stale.contains(&rginx_core::CacheUseStaleCondition::Timeout))
                    || stale_if_error_window_open(entry, now)
            }
            CacheStaleReason::Status(status) => {
                (phase == CacheEntryLifecyclePhase::Grace
                    && use_stale_matches_status(&self.policy.use_stale, status))
                    || (status.is_server_error() && stale_if_error_window_open(entry, now))
            }
        }
    }

    pub(crate) async fn serve_stale(&self, cache_status: CacheStatus) -> Option<HttpResponse> {
        let Some(entry) = &self.cached_entry else {
            return None;
        };
        let Some(response_head) = &self.cached_response_head else {
            return None;
        };
        let response = {
            let _io_guard = self.zone.io_read(&entry.hash).await;
            let paths = cache_paths_for_zone(self.zone.config.as_ref(), &entry.hash);
            build_cached_response_for_request(
                &paths.body,
                response_head.as_ref(),
                &self.request,
                &self.policy,
                self.read_cached_body,
            )
            .await
        };
        match response {
            Ok(response) => {
                if cache_status == CacheStatus::Updating {
                    self.zone.record_updating();
                } else {
                    self.zone.record_stale();
                }
                Some(with_cache_status(response, cache_status))
            }
            Err(error) => {
                tracing::warn!(
                    zone = %self.zone.config.name,
                    key = %self.key,
                    %error,
                    "failed to serve stale cache entry"
                );
                remove_cache_entry_if_matches(&self.zone, &self.key, entry).await;
                None
            }
        }
    }
}
