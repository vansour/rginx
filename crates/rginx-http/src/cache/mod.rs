//! Route-level HTTP response cache for proxied responses.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::SystemTime;

use http::header::{HeaderMap, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH};
use http::{Method, Request, StatusCode, Uri};
use rginx_core::{CacheZone, ConfigSnapshot, Error, Result, RouteCachePolicy};
use serde::{Deserialize, Serialize};
use tokio::sync::futures::OwnedNotified;
use tokio::sync::{Mutex as AsyncMutex, Notify};

use crate::handler::{HttpBody, HttpResponse};

mod entry;
mod fill;
mod index;
mod invalidation;
mod io;
mod load;
mod lookup;
mod manager;
mod policy;
mod request;
mod runtime;
mod shared;
mod state;
mod store;
mod vary;

use entry::{
    CacheMetadata, build_cached_response_for_request, cache_paths_for_zone,
    load_cached_response_head, read_cached_response_for_request, unix_time_ms,
};
#[cfg(test)]
use entry::{
    cache_key_hash, cache_metadata, cache_paths, cache_variant_key, prepare_cached_response_head,
    read_cache_metadata, write_cache_entry,
};
use fill::{CacheFillReadState, ExternalCacheFillReadState, SharedFillExternalStateHandle};
use io::CacheIoLockPool;
#[cfg(test)]
use io::cache_io_lock_stripe;
#[cfg(test)]
use load::load_index_from_disk;
use policy::{header_value, request_requires_revalidation};
#[cfg(test)]
use policy::{response_is_storable, response_ttl};
use request::{cache_request_bypass, render_cache_key};
use runtime::PurgeSelector;
pub(crate) use runtime::with_cache_status;
use runtime::{
    build_conditional_headers, remove_cache_entry_if_matches, remove_cache_files_if_unreferenced,
    remove_cache_files_locked,
};
use shared::{SharedIndexStore, bootstrap_shared_index, sync_zone_shared_index_if_needed};
pub(crate) use state::CacheStaleReason;
use state::{
    CacheAccessScheduleEntry, CacheAccessScheduleTicket, CacheConditionalHeaders,
    CacheEntryHotState, CacheFillGuard, CacheFillLockState, CacheIndex, CacheIndexEntry,
    CacheIndexEntryKind, CacheInvalidationRule, CacheInvalidationSelector, CacheZoneRuntime,
    CacheZoneStats, CachedVaryHeaderValue, FillLockDecision, LookupDecision, LookupWait,
    PreparedCacheResponseHead,
};
pub(crate) use store::CacheStoreError;
#[cfg(test)]
use store::lock_index;
use store::{
    cleanup_inactive_entries_in_zone, purge_zone_entries, read_index,
    refresh_not_modified_response, remove_zone_index_entry_if_matches, store_response,
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
    request: CacheRequest,
    base_key: String,
    key: String,
    cache_status: CacheStatus,
    store_response: bool,
    _fill_guard: Option<CacheFillGuard>,
    cached_entry: Option<CacheIndexEntry>,
    cached_response_head: Option<Arc<PreparedCacheResponseHead>>,
    revalidating: bool,
    request_forces_revalidation: bool,
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
    Updating(HttpResponse, Box<CacheStoreContext>),
    Bypass(CacheStatus),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheStatus {
    Hit,
    Miss,
    Bypass,
    Expired,
    Stale,
    Updating,
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
    pub updating_total: u64,
    pub revalidated_total: u64,
    pub write_success_total: u64,
    pub write_error_total: u64,
    pub eviction_total: u64,
    pub purge_total: u64,
    pub invalidation_total: u64,
    pub inactive_cleanup_total: u64,
    pub active_invalidation_rules: usize,
    pub shared_index_enabled: bool,
    pub shared_index_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachePurgeResult {
    pub zone_name: String,
    pub scope: String,
    pub removed_entries: usize,
    pub removed_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheInvalidationResult {
    pub zone_name: String,
    pub scope: String,
    pub affected_entries: usize,
    pub affected_bytes: usize,
    pub active_rules: usize,
}

impl CacheStatus {
    fn as_header_value(self) -> HeaderValue {
        HeaderValue::from_static(match self {
            Self::Hit => "HIT",
            Self::Miss => "MISS",
            Self::Bypass => "BYPASS",
            Self::Expired => "EXPIRED",
            Self::Stale => "STALE",
            Self::Updating => "UPDATING",
            Self::Revalidated => "REVALIDATED",
        })
    }
}

#[cfg(test)]
mod tests;
