//! Route-level HTTP response cache for proxied responses.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::SystemTime;

use http::header::{ETAG, HeaderMap, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use http::{Method, Request, StatusCode, Uri};
use rginx_core::{CacheZone, ConfigSnapshot, Error, Result, RouteCachePolicy};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::sync::{Mutex as AsyncMutex, Notify};

use crate::handler::{HttpBody, HttpResponse};

mod entry;
mod load;
mod policy;
mod request;
mod store;

#[cfg(test)]
use entry::{cache_key_hash, cache_metadata, write_cache_entry};
use entry::{
    CacheMetadata, build_cached_response, cache_paths, read_cache_metadata, read_cached_response,
    unix_time_ms,
};
use load::load_index_from_disk;
use policy::{header_value, request_requires_revalidation};
#[cfg(test)]
use policy::{response_is_storable, response_ttl};
use request::{cache_request_bypass, render_cache_key};
use store::{
    CacheStoreError, cleanup_inactive_entries_in_zone, lock_index, purge_zone_entries,
    refresh_not_modified_response, remove_index_entry, store_response,
};

const CACHE_STATUS_HEADER: &str = "x-cache";

pub(crate) type CacheChangeNotifier = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone, Default)]
pub(crate) struct CacheManager {
    zones: Arc<HashMap<String, Arc<CacheZoneRuntime>>>,
}

pub(crate) struct CacheStoreContext {
    zone: Arc<CacheZoneRuntime>,
    policy: RouteCachePolicy,
    key: String,
    cache_status: CacheStatus,
    store_response: bool,
    _fill_guard: Option<CacheFillGuard>,
    cached_entry: Option<CacheIndexEntry>,
    cached_metadata: Option<CacheMetadata>,
    allow_stale_on_error: bool,
    revalidating: bool,
    conditional_headers: Option<CacheConditionalHeaders>,
    read_cached_body: bool,
}

#[derive(Clone)]
pub(crate) struct CacheRequest {
    method: Method,
    uri: Uri,
    headers: HeaderMap,
}

pub(crate) enum CacheLookup {
    Hit(HttpResponse),
    Miss(CacheStoreContext),
    Bypass(CacheStatus),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheStatus {
    Hit,
    Miss,
    Bypass,
    Expired,
    Stale,
    Revalidated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheZoneRuntimeSnapshot {
    pub zone_name: String,
    pub path: PathBuf,
    pub max_size_bytes: Option<usize>,
    pub inactive_secs: u64,
    pub default_ttl_secs: u64,
    pub max_entry_bytes: usize,
    pub entry_count: usize,
    pub current_size_bytes: usize,
    pub hit_total: u64,
    pub miss_total: u64,
    pub bypass_total: u64,
    pub expired_total: u64,
    pub stale_total: u64,
    pub revalidated_total: u64,
    pub write_success_total: u64,
    pub write_error_total: u64,
    pub eviction_total: u64,
    pub purge_total: u64,
    pub inactive_cleanup_total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachePurgeResult {
    pub zone_name: String,
    pub scope: String,
    pub removed_entries: usize,
    pub removed_bytes: usize,
}

impl CacheStatus {
    fn as_header_value(self) -> HeaderValue {
        HeaderValue::from_static(match self {
            Self::Hit => "HIT",
            Self::Miss => "MISS",
            Self::Bypass => "BYPASS",
            Self::Expired => "EXPIRED",
            Self::Stale => "STALE",
            Self::Revalidated => "REVALIDATED",
        })
    }
}

struct CacheZoneRuntime {
    config: Arc<CacheZone>,
    index: Mutex<CacheIndex>,
    io_lock: AsyncMutex<()>,
    fill_locks: Arc<Mutex<HashMap<String, Arc<Notify>>>>,
    stats: CacheZoneStats,
    change_notifier: Option<CacheChangeNotifier>,
}

#[derive(Default)]
struct CacheIndex {
    entries: HashMap<String, CacheIndexEntry>,
    current_size_bytes: usize,
}

#[derive(Clone)]
struct CacheIndexEntry {
    hash: String,
    body_size_bytes: usize,
    expires_at_unix_ms: u64,
    stale_if_error_until_unix_ms: Option<u64>,
    stale_while_revalidate_until_unix_ms: Option<u64>,
    must_revalidate: bool,
    last_access_unix_ms: u64,
}

#[derive(Default)]
struct CacheZoneStats {
    hit_total: AtomicU64,
    miss_total: AtomicU64,
    bypass_total: AtomicU64,
    expired_total: AtomicU64,
    stale_total: AtomicU64,
    revalidated_total: AtomicU64,
    write_success_total: AtomicU64,
    write_error_total: AtomicU64,
    eviction_total: AtomicU64,
    purge_total: AtomicU64,
    inactive_cleanup_total: AtomicU64,
}

struct CacheFillGuard {
    key: String,
    fill_locks: Weak<Mutex<HashMap<String, Arc<Notify>>>>,
    notify: Arc<Notify>,
}

#[derive(Clone)]
struct CacheConditionalHeaders {
    if_none_match: Option<HeaderValue>,
    if_modified_since: Option<HeaderValue>,
}

enum LookupDecision {
    FreshHit(CacheIndexEntry),
    StaleWhileRevalidate(CacheIndexEntry),
    Miss {
        cached_entry: Option<CacheIndexEntry>,
        fill_guard: CacheFillGuard,
        cache_status: CacheStatus,
        allow_stale_on_error: bool,
    },
    Wait(Arc<Notify>),
}

impl CacheManager {
    pub(crate) fn from_config_with_notifier(
        config: &ConfigSnapshot,
        change_notifier: Option<CacheChangeNotifier>,
    ) -> Result<Self> {
        let zones = config
            .cache_zones
            .iter()
            .map(|(name, zone)| {
                std::fs::create_dir_all(&zone.path).map_err(|error| {
                    Error::Server(format!(
                        "failed to create cache zone `{name}` directory `{}`: {error}",
                        zone.path.display()
                    ))
                })?;
                let index = load_index_from_disk(zone.as_ref()).map_err(|error| {
                    Error::Server(format!(
                        "failed to load cache zone `{name}` index from `{}`: {error}",
                        zone.path.display()
                    ))
                })?;
                Ok((
                    name.clone(),
                    Arc::new(CacheZoneRuntime {
                        config: zone.clone(),
                        index: Mutex::new(index),
                        io_lock: AsyncMutex::new(()),
                        fill_locks: Arc::new(Mutex::new(HashMap::new())),
                        stats: CacheZoneStats::default(),
                        change_notifier: change_notifier.clone(),
                    }),
                ))
            })
            .collect::<Result<HashMap<_, _>>>()?;

        Ok(Self { zones: Arc::new(zones) })
    }

    pub(crate) async fn lookup(
        &self,
        request: CacheRequest,
        downstream_scheme: &str,
        policy: &RouteCachePolicy,
    ) -> CacheLookup {
        let Some(zone) = self.zones.get(&policy.zone).cloned() else {
            tracing::warn!(
                zone = %policy.zone,
                "cache policy references unknown zone; bypassing cache"
            );
            return CacheLookup::Bypass(CacheStatus::Bypass);
        };

        if cache_request_bypass(&request, policy) {
            zone.record_bypass();
            return CacheLookup::Bypass(CacheStatus::Bypass);
        }

        let key = render_cache_key(
            &request.method,
            &request.uri,
            &request.headers,
            downstream_scheme,
            policy,
        );
        let request_forces_revalidation = request_requires_revalidation(&request.headers);
        let read_cached_body = request.method != Method::HEAD;

        loop {
            let now = unix_time_ms(SystemTime::now());
            match self.lookup_decision(
                &zone,
                &key,
                now,
                request_forces_revalidation,
            ) {
                LookupDecision::FreshHit(entry) => {
                    let cached_response = {
                        let _io_guard = zone.io_lock.lock().await;
                        read_cached_response(&zone, &entry, read_cached_body).await
                    };
                    match cached_response {
                        Ok(mut response) => {
                            zone.record_hit();
                            response
                                .headers_mut()
                                .insert(CACHE_STATUS_HEADER, CacheStatus::Hit.as_header_value());
                            return CacheLookup::Hit(response);
                        }
                        Err(error) => {
                            tracing::warn!(
                                zone = %zone.config.name,
                                key_hash = %entry.hash,
                                %error,
                                "failed to read cached response; treating as miss"
                            );
                            remove_index_entry(&zone, &key);
                            remove_cache_files_if_unindexed(&zone, &key, &entry.hash).await;
                        }
                    }
                }
                LookupDecision::StaleWhileRevalidate(entry) => {
                    match self
                        .stale_response_from_entry(&zone, &key, &entry, read_cached_body)
                        .await
                    {
                        Some(response) => return CacheLookup::Hit(response),
                        None => continue,
                    }
                }
                LookupDecision::Wait(notify) => {
                    notify.notified().await;
                }
                LookupDecision::Miss {
                    cached_entry,
                    fill_guard,
                    cache_status,
                    allow_stale_on_error,
                } => {
                    let (cached_metadata, conditional_headers) = if let Some(entry) = &cached_entry {
                        match self.load_lookup_metadata(&zone, &key, entry).await {
                            Some((metadata, conditional_headers)) => {
                                (Some(metadata), conditional_headers)
                            }
                            None => {
                                drop(fill_guard);
                                continue;
                            }
                        }
                    } else {
                        (None, None)
                    };
                    if cache_status == CacheStatus::Miss {
                        zone.record_miss();
                    } else if cache_status == CacheStatus::Expired {
                        zone.record_expired();
                    }

                    return CacheLookup::Miss(CacheStoreContext {
                        zone,
                        policy: policy.clone(),
                        key,
                        cache_status,
                        store_response: request.method == Method::GET,
                        _fill_guard: Some(fill_guard),
                        cached_entry,
                        cached_metadata,
                        allow_stale_on_error,
                        revalidating: cache_status == CacheStatus::Revalidated,
                        conditional_headers,
                        read_cached_body,
                    });
                }
            }
        }
    }

    pub(crate) async fn store_response(
        &self,
        context: CacheStoreContext,
        response: HttpResponse,
    ) -> HttpResponse {
        let status = context.cache_status;
        let response = if context.store_response {
            match store_response(context, response).await {
                Ok(response) => response,
                Err(error) => {
                    tracing::warn!(%error, "failed to store cached response");
                    crate::handler::text_response(
                        StatusCode::BAD_GATEWAY,
                        "text/plain; charset=utf-8",
                        format!("failed to read upstream response while caching: {error}\n"),
                    )
                }
            }
        } else {
            response
        };

        with_cache_status(response, status)
    }

    pub(crate) async fn complete_not_modified(
        &self,
        context: CacheStoreContext,
        response: HttpResponse,
    ) -> std::result::Result<HttpResponse, CacheStoreError> {
        refresh_not_modified_response(context, response).await
    }

    pub(crate) fn snapshot(&self) -> Vec<CacheZoneRuntimeSnapshot> {
        let mut snapshots = self
            .zones
            .values()
            .map(|zone| zone.snapshot())
            .collect::<Vec<_>>();
        snapshots.sort_by(|left, right| left.zone_name.cmp(&right.zone_name));
        snapshots
    }

    pub(crate) async fn cleanup_inactive_entries(&self) {
        for zone in self.zones.values() {
            cleanup_inactive_entries_in_zone(zone).await;
        }
    }

    pub(crate) async fn purge_zone(
        &self,
        zone_name: &str,
    ) -> std::result::Result<CachePurgeResult, String> {
        let zone = self
            .zones
            .get(zone_name)
            .cloned()
            .ok_or_else(|| format!("unknown cache zone `{zone_name}`"))?;
        Ok(purge_zone_entries(zone, PurgeSelector::All).await)
    }

    pub(crate) async fn purge_key(
        &self,
        zone_name: &str,
        key: &str,
    ) -> std::result::Result<CachePurgeResult, String> {
        let zone = self
            .zones
            .get(zone_name)
            .cloned()
            .ok_or_else(|| format!("unknown cache zone `{zone_name}`"))?;
        Ok(purge_zone_entries(zone, PurgeSelector::Exact(key.to_string())).await)
    }

    pub(crate) async fn purge_prefix(
        &self,
        zone_name: &str,
        prefix: &str,
    ) -> std::result::Result<CachePurgeResult, String> {
        let zone = self
            .zones
            .get(zone_name)
            .cloned()
            .ok_or_else(|| format!("unknown cache zone `{zone_name}`"))?;
        Ok(purge_zone_entries(zone, PurgeSelector::Prefix(prefix.to_string())).await)
    }

    fn lookup_decision(
        &self,
        zone: &Arc<CacheZoneRuntime>,
        key: &str,
        now: u64,
        request_forces_revalidation: bool,
    ) -> LookupDecision {
        let mut index = lock_index(&zone.index);
        match index.entries.get_mut(key) {
            Some(entry)
                if now <= entry.expires_at_unix_ms
                    && !entry.must_revalidate
                    && !request_forces_revalidation =>
            {
                entry.last_access_unix_ms = now;
                LookupDecision::FreshHit(entry.clone())
            }
            Some(entry) => {
                entry.last_access_unix_ms = now;
                if let Some(fill_guard) = zone.try_acquire_fill_guard(key) {
                    let expired = now > entry.expires_at_unix_ms;
                    let cache_status = if expired {
                        CacheStatus::Expired
                    } else {
                        CacheStatus::Revalidated
                    };
                    let allow_stale_on_error = expired
                        && entry
                            .stale_if_error_until_unix_ms
                            .is_some_and(|until| now <= until);
                    LookupDecision::Miss {
                        cached_entry: Some(entry.clone()),
                        fill_guard,
                        cache_status,
                        allow_stale_on_error,
                    }
                } else if now > entry.expires_at_unix_ms
                    && entry
                        .stale_while_revalidate_until_unix_ms
                        .is_some_and(|until| now <= until)
                {
                    LookupDecision::StaleWhileRevalidate(entry.clone())
                } else {
                    LookupDecision::Wait(
                        zone.current_fill_notify(key)
                            .expect("fill lock should still exist when waiting"),
                    )
                }
            }
            None => {
                if let Some(fill_guard) = zone.try_acquire_fill_guard(key) {
                    LookupDecision::Miss {
                        cached_entry: None,
                        fill_guard,
                        cache_status: CacheStatus::Miss,
                        allow_stale_on_error: false,
                    }
                } else {
                    LookupDecision::Wait(
                        zone.current_fill_notify(key)
                            .expect("fill lock should still exist when waiting"),
                    )
                }
            }
        }
    }

    async fn load_lookup_metadata(
        &self,
        zone: &Arc<CacheZoneRuntime>,
        key: &str,
        entry: &CacheIndexEntry,
    ) -> Option<(CacheMetadata, Option<CacheConditionalHeaders>)> {
        let metadata = {
            let _io_guard = zone.io_lock.lock().await;
            let paths = cache_paths(&zone.config.path, &entry.hash);
            read_cache_metadata(&paths.metadata).await
        };
        let metadata = match metadata {
            Ok(metadata) => metadata,
            Err(error) => {
                tracing::warn!(
                    zone = %zone.config.name,
                    key_hash = %entry.hash,
                    %error,
                    "failed to read cache metadata; removing entry"
                );
                remove_index_entry(zone, key);
                remove_cache_files_if_unindexed(zone, key, &entry.hash).await;
                return None;
            }
        };
        let headers = match metadata.headers_map() {
            Ok(headers) => headers,
            Err(error) => {
                tracing::warn!(
                    zone = %zone.config.name,
                    key_hash = %entry.hash,
                    %error,
                    "failed to decode cached response headers; removing entry"
                );
                remove_index_entry(zone, key);
                remove_cache_files_if_unindexed(zone, key, &entry.hash).await;
                return None;
            }
        };
        let conditional_headers = build_conditional_headers(&headers);
        Some((metadata, conditional_headers))
    }

    async fn stale_response_from_entry(
        &self,
        zone: &Arc<CacheZoneRuntime>,
        key: &str,
        entry: &CacheIndexEntry,
        read_cached_body: bool,
    ) -> Option<HttpResponse> {
        let (metadata, response) = {
            let _io_guard = zone.io_lock.lock().await;
            let paths = cache_paths(&zone.config.path, &entry.hash);
            let metadata = read_cache_metadata(&paths.metadata).await.ok()?;
            let response = build_cached_response(&paths.body, &metadata, read_cached_body).await.ok()?;
            (metadata, response)
        };
        if metadata.key != key {
            remove_index_entry(zone, key);
            remove_cache_files_if_unindexed(zone, key, &entry.hash).await;
            return None;
        }
        zone.record_stale();
        Some(with_cache_status(response, CacheStatus::Stale))
    }
}

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
        self.revalidating && status == StatusCode::NOT_MODIFIED
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
    fn snapshot(&self) -> CacheZoneRuntimeSnapshot {
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

    fn try_acquire_fill_guard(self: &Arc<Self>, key: &str) -> Option<CacheFillGuard> {
        let mut fill_locks =
            self.fill_locks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if fill_locks.contains_key(key) {
            return None;
        }
        let notify = Arc::new(Notify::new());
        fill_locks.insert(key.to_string(), notify.clone());
        Some(CacheFillGuard {
            key: key.to_string(),
            fill_locks: Arc::downgrade(&self.fill_locks),
            notify,
        })
    }

    fn current_fill_notify(&self, key: &str) -> Option<Arc<Notify>> {
        self.fill_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(key)
            .cloned()
    }

    fn record_hit(&self) {
        self.record_counter(&self.stats.hit_total, 1);
    }

    fn record_miss(&self) {
        self.record_counter(&self.stats.miss_total, 1);
    }

    fn record_bypass(&self) {
        self.record_counter(&self.stats.bypass_total, 1);
    }

    fn record_expired(&self) {
        self.record_counter(&self.stats.expired_total, 1);
    }

    fn record_stale(&self) {
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
        self.notify_changed();
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
            fill_locks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&self.key);
        }
        self.notify.notify_waiters();
    }
}

pub(crate) fn with_cache_status(mut response: HttpResponse, status: CacheStatus) -> HttpResponse {
    response.headers_mut().insert(CACHE_STATUS_HEADER, status.as_header_value());
    response
}

async fn remove_cache_files_if_unindexed(zone: &CacheZoneRuntime, key: &str, hash: &str) {
    let _io_guard = zone.io_lock.lock().await;
    if lock_index(&zone.index).entries.contains_key(key) {
        return;
    }
    let paths = cache_paths(&zone.config.path, hash);
    let _ = fs::remove_file(paths.metadata).await;
    let _ = fs::remove_file(paths.body).await;
}

fn build_conditional_headers(headers: &HeaderMap) -> Option<CacheConditionalHeaders> {
    let if_none_match = header_value(headers, ETAG)
        .and_then(|value| HeaderValue::from_str(&value).ok());
    let if_modified_since = header_value(headers, LAST_MODIFIED)
        .and_then(|value| HeaderValue::from_str(&value).ok());
    (if_none_match.is_some() || if_modified_since.is_some()).then_some(CacheConditionalHeaders {
        if_none_match,
        if_modified_since,
    })
}

#[derive(Debug, Clone)]
pub(super) enum PurgeSelector {
    All,
    Exact(String),
    Prefix(String),
}

#[cfg(test)]
mod tests;
