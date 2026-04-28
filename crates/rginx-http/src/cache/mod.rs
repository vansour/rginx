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
use tokio::sync::futures::OwnedNotified;
use tokio::sync::{Mutex as AsyncMutex, Notify};

use crate::handler::{HttpBody, HttpResponse};

mod entry;
mod load;
mod lookup;
mod manager;
mod policy;
mod request;
mod runtime;
mod store;

use entry::{
    CacheMetadata, build_cached_response, cache_paths, read_cache_metadata, read_cached_response,
    unix_time_ms,
};
#[cfg(test)]
use entry::{cache_key_hash, cache_metadata, write_cache_entry};
use load::load_index_from_disk;
use policy::{header_value, request_requires_revalidation};
#[cfg(test)]
use policy::{response_is_storable, response_ttl};
use request::{cache_request_bypass, render_cache_key};
use runtime::PurgeSelector;
pub(crate) use runtime::with_cache_status;
use runtime::{build_conditional_headers, remove_cache_files_if_unindexed};
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
    Miss(Box<CacheStoreContext>),
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
    Wait(OwnedNotified),
}

enum FillLockDecision {
    Acquired(CacheFillGuard),
    Wait(OwnedNotified),
}

#[cfg(test)]
mod tests;
