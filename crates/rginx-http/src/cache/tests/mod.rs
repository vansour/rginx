use std::collections::HashMap;
use std::path::PathBuf;
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

fn test_zone(path: PathBuf, max_entry_bytes: usize) -> Arc<CacheZoneRuntime> {
    test_zone_with_notifier(path, max_entry_bytes, None)
}

fn test_zone_with_notifier(
    path: PathBuf,
    max_entry_bytes: usize,
    change_notifier: Option<CacheChangeNotifier>,
) -> Arc<CacheZoneRuntime> {
    Arc::new(CacheZoneRuntime {
        config: Arc::new(CacheZone {
            name: "default".to_string(),
            path,
            max_size_bytes: Some(1024 * 1024),
            inactive: Duration::from_secs(60),
            default_ttl: Duration::from_secs(60),
            max_entry_bytes,
        }),
        index: Mutex::new(CacheIndex::default()),
        io_lock: AsyncMutex::new(()),
        fill_locks: Arc::new(Mutex::new(HashMap::new())),
        stats: CacheZoneStats::default(),
        change_notifier,
    })
}

fn test_index_entry(
    hash: String,
    body_size_bytes: usize,
    expires_at_unix_ms: u64,
    last_access_unix_ms: u64,
) -> CacheIndexEntry {
    CacheIndexEntry {
        hash,
        body_size_bytes,
        expires_at_unix_ms,
        stale_if_error_until_unix_ms: None,
        stale_while_revalidate_until_unix_ms: None,
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
            key: rginx_core::CacheKeyTemplate::parse("{uri}").expect("key should parse"),
            stale_if_error: None,
        },
        key: key.to_string(),
        cache_status: CacheStatus::Miss,
        store_response: true,
        _fill_guard: None,
        cached_entry: None,
        cached_metadata: None,
        allow_stale_on_error: false,
        revalidating: false,
        conditional_headers: None,
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
        key: rginx_core::CacheKeyTemplate::parse("{scheme}:{host}:{uri}")
            .expect("key should parse"),
        stale_if_error: None,
    }
}

fn test_metadata_input(
    stored_at_unix_ms: u64,
    expires_at_unix_ms: u64,
    body_size_bytes: usize,
) -> CacheMetadataInput {
    CacheMetadataInput {
        stored_at_unix_ms,
        expires_at_unix_ms,
        stale_if_error_until_unix_ms: None,
        stale_while_revalidate_until_unix_ms: None,
        must_revalidate: false,
        body_size_bytes,
    }
}
