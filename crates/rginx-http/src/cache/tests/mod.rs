use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use http::{Method, StatusCode};
use rginx_core::{CacheZone, RouteCachePolicy};

use super::entry::CacheMetadataInput;
use super::*;

mod lookup;
mod notifications;
mod policy;
mod storage;
mod storage_p1;
mod storage_p2;
mod storage_p3;
mod storage_regressions;
mod stress;

fn test_zone(path: PathBuf, max_entry_bytes: usize) -> Arc<CacheZoneRuntime> {
    test_zone_with_notifier(path, max_entry_bytes, None)
}

fn test_zone_with_notifier(
    path: PathBuf,
    max_entry_bytes: usize,
    change_notifier: Option<CacheChangeNotifier>,
) -> Arc<CacheZoneRuntime> {
    let config = Arc::new(CacheZone {
        name: "default".to_string(),
        path,
        max_size_bytes: Some(1024 * 1024),
        inactive: Duration::from_secs(60),
        default_ttl: Duration::from_secs(60),
        max_entry_bytes,
        path_levels: vec![2],
        loader_batch_entries: 100,
        loader_sleep: Duration::ZERO,
        manager_batch_entries: 100,
        manager_sleep: Duration::ZERO,
        inactive_cleanup_interval: Duration::from_secs(60),
        shared_index: true,
    });
    Arc::new(CacheZoneRuntime {
        config: config.clone(),
        index: Mutex::new(CacheIndex::default()),
        io_locks: CacheIoLockPool::new(),
        shared_index_sync_lock: AsyncMutex::new(()),
        shared_index_store: Some(Arc::new(shared::shared_index_store(config.as_ref()))),
        fill_locks: Arc::new(Mutex::new(HashMap::new())),
        fill_lock_generation: AtomicU64::new(0),
        last_inactive_cleanup_unix_ms: AtomicU64::new(0),
        shared_index_generation: AtomicU64::new(0),
        stats: CacheZoneStats::default(),
        change_notifier,
    })
}

fn test_index_entry(
    base_key: &str,
    hash: String,
    body_size_bytes: usize,
    expires_at_unix_ms: u64,
    last_access_unix_ms: u64,
) -> CacheIndexEntry {
    CacheIndexEntry {
        hash,
        base_key: base_key.to_string(),
        vary: Vec::new(),
        body_size_bytes,
        expires_at_unix_ms,
        stale_if_error_until_unix_ms: None,
        stale_while_revalidate_until_unix_ms: None,
        requires_revalidation: false,
        must_revalidate: false,
        last_access_unix_ms,
    }
}

fn test_store_context(zone: Arc<CacheZoneRuntime>, key: &str) -> CacheStoreContext {
    CacheStoreContext {
        zone,
        policy: RouteCachePolicy {
            zone: "default".to_string(),
            methods: vec![Method::GET],
            statuses: vec![StatusCode::OK],
            ttl_by_status: Vec::new(),
            key: rginx_core::CacheKeyTemplate::parse("{uri}").expect("key should parse"),
            cache_bypass: None,
            no_cache: None,
            stale_if_error: None,
            use_stale: Vec::new(),
            background_update: false,
            lock_timeout: Duration::from_secs(5),
            lock_age: Duration::from_secs(5),
            min_uses: 1,
            ignore_headers: Vec::new(),
            range_requests: rginx_core::CacheRangeRequestPolicy::Bypass,
            slice_size_bytes: None,
            convert_head: true,
        },
        request: CacheRequest {
            method: Method::GET,
            uri: http::Uri::from_static("/"),
            headers: http::HeaderMap::new(),
        },
        base_key: key.to_string(),
        key: key.to_string(),
        cache_status: CacheStatus::Miss,
        store_response: true,
        _fill_guard: None,
        cached_entry: None,
        cached_metadata: None,
        revalidating: false,
        conditional_headers: None,
        request_forces_revalidation: false,
        read_cached_body: true,
    }
}

fn test_manager(path: PathBuf, max_entry_bytes: usize) -> CacheManager {
    CacheManager {
        zones: Arc::new(HashMap::from([("default".to_string(), test_zone(path, max_entry_bytes))])),
    }
}

fn test_policy() -> RouteCachePolicy {
    RouteCachePolicy {
        zone: "default".to_string(),
        methods: vec![Method::GET, Method::HEAD],
        statuses: vec![StatusCode::OK],
        ttl_by_status: Vec::new(),
        key: rginx_core::CacheKeyTemplate::parse("{scheme}:{host}:{uri}")
            .expect("key should parse"),
        cache_bypass: None,
        no_cache: None,
        stale_if_error: None,
        use_stale: Vec::new(),
        background_update: false,
        lock_timeout: Duration::from_secs(5),
        lock_age: Duration::from_secs(5),
        min_uses: 1,
        ignore_headers: Vec::new(),
        range_requests: rginx_core::CacheRangeRequestPolicy::Bypass,
        slice_size_bytes: None,
        convert_head: true,
    }
}

fn test_metadata_input(
    base_key: &str,
    stored_at_unix_ms: u64,
    expires_at_unix_ms: u64,
    body_size_bytes: usize,
) -> CacheMetadataInput {
    CacheMetadataInput {
        base_key: base_key.to_string(),
        vary: Vec::new(),
        stored_at_unix_ms,
        expires_at_unix_ms,
        stale_if_error_until_unix_ms: None,
        stale_while_revalidate_until_unix_ms: None,
        requires_revalidation: false,
        must_revalidate: false,
        body_size_bytes,
    }
}
