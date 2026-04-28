use super::*;

impl CacheRequest {
    pub(crate) fn from_request(request: &Request<HttpBody>) -> Self {
        Self {
            method: request.method().clone(),
            uri: request.uri().clone(),
            headers: request.headers().clone(),
        }
    }
}

impl CacheStoreContext {
    pub(crate) fn cache_status(&self) -> CacheStatus {
        self.cache_status
    }

    pub(crate) fn apply_conditional_request_headers(&self, headers: &mut HeaderMap) {
        let Some(conditional_headers) = &self.conditional_headers else {
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

    pub(crate) fn can_serve_stale_on_error(&self) -> bool {
        self.allow_stale_on_error && self.cached_entry.is_some() && self.cached_metadata.is_some()
    }

    pub(crate) async fn serve_stale_on_error(&self) -> Option<HttpResponse> {
        let Some(entry) = &self.cached_entry else {
            return None;
        };
        let Some(metadata) = &self.cached_metadata else {
            return None;
        };
        let response = {
            let _io_guard = self.zone.io_lock.lock().await;
            let paths = cache_paths(&self.zone.config.path, &entry.hash);
            build_cached_response(&paths.body, metadata, self.read_cached_body).await
        };
        match response {
            Ok(response) => {
                self.zone.record_stale();
                Some(with_cache_status(response, CacheStatus::Stale))
            }
            Err(error) => {
                tracing::warn!(
                    zone = %self.zone.config.name,
                    key = %self.key,
                    %error,
                    "failed to serve stale cache entry"
                );
                remove_index_entry(&self.zone, &self.key);
                remove_cache_files_if_unindexed(&self.zone, &self.key, &entry.hash).await;
                None
            }
        }
    }
}

impl CacheZoneRuntime {
    pub(super) fn snapshot(&self) -> CacheZoneRuntimeSnapshot {
        let index = lock_index(&self.index);
        CacheZoneRuntimeSnapshot {
            zone_name: self.config.name.clone(),
            path: self.config.path.clone(),
            max_size_bytes: self.config.max_size_bytes,
            inactive_secs: self.config.inactive.as_secs(),
            default_ttl_secs: self.config.default_ttl.as_secs(),
            max_entry_bytes: self.config.max_entry_bytes,
            entry_count: index.entries.len(),
            current_size_bytes: index.current_size_bytes,
            hit_total: self.stats.hit_total.load(Ordering::Relaxed),
            miss_total: self.stats.miss_total.load(Ordering::Relaxed),
            bypass_total: self.stats.bypass_total.load(Ordering::Relaxed),
            expired_total: self.stats.expired_total.load(Ordering::Relaxed),
            stale_total: self.stats.stale_total.load(Ordering::Relaxed),
            revalidated_total: self.stats.revalidated_total.load(Ordering::Relaxed),
            write_success_total: self.stats.write_success_total.load(Ordering::Relaxed),
            write_error_total: self.stats.write_error_total.load(Ordering::Relaxed),
            eviction_total: self.stats.eviction_total.load(Ordering::Relaxed),
            purge_total: self.stats.purge_total.load(Ordering::Relaxed),
            inactive_cleanup_total: self.stats.inactive_cleanup_total.load(Ordering::Relaxed),
        }
    }

    pub(super) fn fill_lock_decision(self: &Arc<Self>, key: &str) -> FillLockDecision {
        let mut fill_locks =
            self.fill_locks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(notify) = fill_locks.get(key).cloned() {
            return FillLockDecision::Wait(notify.notified_owned());
        }
        let notify = Arc::new(Notify::new());
        fill_locks.insert(key.to_string(), notify.clone());
        FillLockDecision::Acquired(CacheFillGuard {
            key: key.to_string(),
            fill_locks: Arc::downgrade(&self.fill_locks),
            notify,
        })
    }

    pub(super) fn record_hit(&self) {
        self.record_counter(&self.stats.hit_total, 1);
    }

    pub(super) fn record_miss(&self) {
        self.record_counter(&self.stats.miss_total, 1);
    }

    pub(super) fn record_bypass(&self) {
        self.record_counter(&self.stats.bypass_total, 1);
    }

    pub(super) fn record_expired(&self) {
        self.record_counter(&self.stats.expired_total, 1);
    }

    pub(super) fn record_stale(&self) {
        self.record_counter(&self.stats.stale_total, 1);
    }

    pub(super) fn record_revalidated(&self) {
        self.record_counter(&self.stats.revalidated_total, 1);
    }

    pub(super) fn record_write_success(&self) {
        self.record_counter(&self.stats.write_success_total, 1);
    }

    pub(super) fn record_write_error(&self) {
        self.record_counter(&self.stats.write_error_total, 1);
    }

    pub(super) fn record_evictions(&self, count: usize) {
        if count > 0 {
            self.record_counter(&self.stats.eviction_total, count as u64);
        }
    }

    pub(super) fn record_purge(&self, count: usize) {
        if count > 0 {
            self.record_counter(&self.stats.purge_total, count as u64);
        }
    }

    pub(super) fn record_inactive_cleanup(&self, count: usize) {
        if count > 0 {
            self.record_counter(&self.stats.inactive_cleanup_total, count as u64);
        }
    }

    fn record_counter(&self, counter: &AtomicU64, value: u64) {
        counter.fetch_add(value, Ordering::Relaxed);
    }

    pub(super) fn notify_changed(&self) {
        if let Some(notifier) = &self.change_notifier {
            notifier(&self.config.name);
        }
    }
}

impl Drop for CacheFillGuard {
    fn drop(&mut self) {
        if let Some(fill_locks) = self.fill_locks.upgrade() {
            fill_locks.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&self.key);
        }
        self.notify.notify_waiters();
    }
}

pub(crate) fn with_cache_status(mut response: HttpResponse, status: CacheStatus) -> HttpResponse {
    response.headers_mut().insert(CACHE_STATUS_HEADER, status.as_header_value());
    response
}

pub(super) async fn remove_cache_files_if_unindexed(
    zone: &CacheZoneRuntime,
    key: &str,
    hash: &str,
) {
    let _io_guard = zone.io_lock.lock().await;
    if lock_index(&zone.index).entries.contains_key(key) {
        return;
    }
    let paths = cache_paths(&zone.config.path, hash);
    let _ = fs::remove_file(paths.metadata).await;
    let _ = fs::remove_file(paths.body).await;
}

pub(super) fn build_conditional_headers(headers: &HeaderMap) -> Option<CacheConditionalHeaders> {
    let if_none_match =
        header_value(headers, ETAG).and_then(|value| HeaderValue::from_str(&value).ok());
    let if_modified_since =
        header_value(headers, LAST_MODIFIED).and_then(|value| HeaderValue::from_str(&value).ok());
    (if_none_match.is_some() || if_modified_since.is_some())
        .then_some(CacheConditionalHeaders { if_none_match, if_modified_since })
}

#[derive(Debug, Clone)]
pub(in crate::cache) enum PurgeSelector {
    All,
    Exact(String),
    Prefix(String),
}
