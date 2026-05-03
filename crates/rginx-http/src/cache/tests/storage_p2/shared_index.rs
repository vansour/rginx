use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use http::header::CACHE_CONTROL;
use http::{Method, Request, Response, StatusCode};
use http_body_util::BodyExt;

use crate::handler::full_body;

use super::*;

fn test_manager_with_max_size(path: std::path::PathBuf, max_size_bytes: usize) -> CacheManager {
    let config = Arc::new(CacheZone {
        name: "default".to_string(),
        path,
        max_size_bytes: Some(max_size_bytes),
        inactive: Duration::from_secs(60),
        default_ttl: Duration::from_secs(60),
        max_entry_bytes: 1024,
        path_levels: vec![2],
        loader_batch_entries: 100,
        loader_sleep: Duration::ZERO,
        manager_batch_entries: 100,
        manager_sleep: Duration::ZERO,
        inactive_cleanup_interval: Duration::from_secs(60),
        shared_index: true,
    });
    let (
        index,
        shared_index_store,
        shared_index_generation,
        shared_index_store_epoch,
        shared_index_change_seq,
    ) = shared::bootstrap_shared_index(config.as_ref())
        .expect("test shared index should bootstrap");
    let zone = Arc::new(CacheZoneRuntime {
        config: config.clone(),
        index: RwLock::new(index),
        hot_entries: RwLock::new(HashMap::new()),
        io_locks: CacheIoLockPool::new(),
        shared_index_sync_lock: AsyncMutex::new(()),
        shared_index_store,
        fill_locks: Arc::new(Mutex::new(HashMap::new())),
        fill_lock_generation: AtomicU64::new(0),
        last_inactive_cleanup_unix_ms: AtomicU64::new(0),
        shared_index_generation: AtomicU64::new(shared_index_generation),
        shared_index_store_epoch: AtomicU64::new(shared_index_store_epoch),
        shared_index_change_seq: AtomicU64::new(shared_index_change_seq),
        stats: CacheZoneStats::default(),
        change_notifier: None,
    });
    CacheManager { zones: Arc::new(HashMap::from([("default".to_string(), zone)])) }
}

#[tokio::test]
async fn shared_index_sync_shares_entries_between_managers() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager_a = test_manager(temp.path().to_path_buf(), 1024);
    let manager_b = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/shared")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    let context =
        match manager_a.lookup(CacheRequest::from_request(&request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("empty cache should miss"),
        };
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("shared"))
        .expect("response should build");
    let stored = manager_a.store_response(context, response).await;
    assert_eq!(stored.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");
    let _ = drain_response(stored).await;

    match manager_b.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            assert_eq!(response.headers().get(CACHE_STATUS_HEADER).unwrap(), "HIT");
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"shared");
        }
        CacheLookup::Miss(_) => panic!("second manager should sync the shared index and hit"),
        CacheLookup::Updating(_, _) => panic!("fresh shared entry should not update"),
        CacheLookup::Bypass(status) => panic!("shared cache request should not bypass: {status:?}"),
    }
}

#[tokio::test]
async fn shared_index_delta_sync_preserves_hot_heads_for_unchanged_keys() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager_a = test_manager(temp.path().to_path_buf(), 1024);
    let manager_b = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let primary_request = Request::builder()
        .method(Method::GET)
        .uri("/shared-delta-primary")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let secondary_request = Request::builder()
        .method(Method::GET)
        .uri("/shared-delta-secondary")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    let primary_context = match manager_a
        .lookup(CacheRequest::from_request(&primary_request), "https", &policy)
        .await
    {
        CacheLookup::Miss(context) => *context,
        _ => panic!("first manager should miss before storing primary entry"),
    };
    let primary_response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("primary"))
        .expect("response should build");
    let _ = drain_response(manager_a.store_response(primary_context, primary_response).await).await;

    match manager_b.lookup(CacheRequest::from_request(&primary_request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"primary");
        }
        _ => panic!("second manager should hit the primary shared entry"),
    }

    let zone_b = manager_b.zones.get("default").expect("default zone should exist");
    let primary_key = "https:example.com:/shared-delta-primary";
    let primary_hash = cache_key_hash(primary_key);
    assert!(zone_b.prepared_response_head(primary_key, &primary_hash).is_some());

    let secondary_context = match manager_a
        .lookup(CacheRequest::from_request(&secondary_request), "https", &policy)
        .await
    {
        CacheLookup::Miss(context) => *context,
        _ => panic!("first manager should miss before storing secondary entry"),
    };
    let secondary_response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("secondary"))
        .expect("response should build");
    let _ =
        drain_response(manager_a.store_response(secondary_context, secondary_response).await).await;
    let _ = wait_for_hit(&manager_a, &secondary_request, &policy).await;

    let primary_paths = cache_paths_for_zone(zone_b.config.as_ref(), &primary_hash);
    tokio::fs::remove_file(&primary_paths.metadata)
        .await
        .expect("primary metadata sidecar should be removed");

    match manager_b.lookup(CacheRequest::from_request(&primary_request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"primary");
        }
        _ => panic!("delta sync should preserve unchanged hot response heads"),
    }

    assert!(zone_b.prepared_response_head(primary_key, &primary_hash).is_some());
    assert!(
        lock_index(&zone_b.index).entries.contains_key("https:example.com:/shared-delta-secondary")
    );
}

#[tokio::test]
async fn shared_index_sync_shares_admission_counts_between_managers() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager_a = test_manager(temp.path().to_path_buf(), 1024);
    let manager_b = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.min_uses = 2;
    let request = Request::builder()
        .method(Method::GET)
        .uri("/min-uses")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    let context_a =
        match manager_a.lookup(CacheRequest::from_request(&request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("first manager should miss"),
        };
    let response_a = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("shared min uses"))
        .expect("response should build");
    let _ = drain_response(manager_a.store_response(context_a, response_a).await).await;

    let context_b =
        match manager_b.lookup(CacheRequest::from_request(&request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("second manager should still miss before admission"),
        };
    let response_b = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("shared min uses"))
        .expect("response should build");
    let _ = drain_response(manager_b.store_response(context_b, response_b).await).await;

    match manager_a.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"shared min uses");
        }
        _ => panic!("shared admission counts should allow the second manager to populate"),
    }
}

#[tokio::test]
async fn shared_index_sync_keeps_local_hits_when_shared_metadata_db_is_corrupted() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager_a = test_manager(temp.path().to_path_buf(), 1024);
    let manager_b = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/shared-corrupt")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    let context =
        match manager_a.lookup(CacheRequest::from_request(&request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("first manager should miss before storing"),
        };
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("shared"))
        .expect("response should build");
    let _ = drain_response(manager_a.store_response(context, response).await).await;

    match manager_b.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"shared");
        }
        _ => panic!("second manager should sync the shared cache entry before corruption"),
    }

    let zone = manager_b.zones.get("default").expect("zone should exist");
    let shared_generation = zone.shared_index_generation.load(Ordering::Relaxed);
    let shared_path = zone
        .shared_index_store
        .as_ref()
        .expect("shared metadata store should exist")
        .path()
        .to_path_buf();
    std::fs::write(&shared_path, b"corrupt sqlite bytes")
        .expect("shared metadata db should be corrupted on disk");

    match manager_b.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"shared");
        }
        _ => panic!("corrupt shared metadata must not evict an already-synced local hit"),
    }

    assert_eq!(zone.shared_index_generation.load(Ordering::Relaxed), shared_generation);
    assert!(lock_index(&zone.index).entries.contains_key("https:example.com:/shared-corrupt"));
}

#[tokio::test]
async fn shared_index_sync_propagates_purged_entries_between_managers() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager_a = test_manager(temp.path().to_path_buf(), 1024);
    let manager_b = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/purged-shared")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    let context =
        match manager_a.lookup(CacheRequest::from_request(&request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("first manager should miss before storing"),
        };
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("purged"))
        .expect("response should build");
    let _ = drain_response(manager_a.store_response(context, response).await).await;

    match manager_b.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Hit(_) => {}
        _ => panic!("second manager should hit the shared cache entry"),
    }

    let zone_b = manager_b.zones.get("default").expect("default zone should exist");
    let purge_key = "https:example.com:/purged-shared";
    let purge_hash = cache_key_hash(purge_key);
    assert!(zone_b.prepared_response_head(purge_key, &purge_hash).is_some());

    let purge = manager_a.purge_key("default", purge_key).await.expect("purge should succeed");
    assert_eq!(purge.removed_entries, 1);

    match manager_b.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(_) => {}
        _ => panic!("purged shared entry should no longer hit on the second manager"),
    }
    assert!(zone_b.prepared_response_head(purge_key, &purge_hash).is_none());
}

#[tokio::test]
async fn shared_index_delta_replay_keeps_eviction_schedule_consistent() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager_a = test_manager_with_max_size(temp.path().to_path_buf(), 6);
    let manager_b = test_manager_with_max_size(temp.path().to_path_buf(), 6);
    let policy = test_policy();
    let request_a = Request::builder()
        .method(Method::GET)
        .uri("/shared-evict-a")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let request_b = Request::builder()
        .method(Method::GET)
        .uri("/shared-evict-b")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let request_c = Request::builder()
        .method(Method::GET)
        .uri("/shared-evict-c")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    for (request, body) in [(&request_a, "aaa"), (&request_b, "bbb")] {
        let context =
            match manager_a.lookup(CacheRequest::from_request(request), "https", &policy).await {
                CacheLookup::Miss(context) => *context,
                _ => panic!("shared key should miss before store"),
            };
        let _ = drain_response(
            manager_a
                .store_response(
                    context,
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(CACHE_CONTROL, "max-age=60")
                        .body(full_body(body))
                        .expect("response should build"),
                )
                .await,
        )
        .await;
        let _ = wait_for_hit(&manager_a, request, &policy).await;
        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    let _ = wait_for_hit(&manager_b, &request_a, &policy).await;
    let _ = wait_for_hit(&manager_b, &request_b, &policy).await;

    {
        let zone_b = manager_b.zones.get("default").expect("default zone should exist");
        let index = lock_index(&zone_b.index);
        assert_eq!(
            index.access_schedule.len(),
            2,
            "unexpected shared schedule after sync: entries={:?} schedule={:?}",
            index.entries.keys().collect::<Vec<_>>(),
            index
                .access_schedule
                .iter()
                .map(|entry| (&entry.key, entry.last_access_unix_ms))
                .collect::<Vec<_>>()
        );
    }

    let context_c =
        match manager_b.lookup(CacheRequest::from_request(&request_c), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("third shared key should miss before store"),
        };
    let _ = drain_response(
        manager_b
            .store_response(
                context_c,
                Response::builder()
                    .status(StatusCode::OK)
                    .header(CACHE_CONTROL, "max-age=60")
                    .body(full_body("ccc"))
                    .expect("response should build"),
            )
            .await,
    )
    .await;
    let _ = wait_for_hit(&manager_b, &request_c, &policy).await;

    let zone_b = manager_b.zones.get("default").expect("default zone should exist");
    let index = lock_index(&zone_b.index);
    assert!(
        !index.entries.contains_key("https:example.com:/shared-evict-a"),
        "unexpected shared keys after eviction: {:?}",
        index.entries.keys().collect::<Vec<_>>()
    );
    assert!(index.entries.contains_key("https:example.com:/shared-evict-b"));
    assert!(index.entries.contains_key("https:example.com:/shared-evict-c"));
}

#[test]
fn bootstrap_shared_index_imports_legacy_sidecar_into_shared_metadata_db() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = CacheZone {
        name: "default".to_string(),
        path: temp.path().to_path_buf(),
        max_size_bytes: Some(1024 * 1024),
        inactive: Duration::from_secs(60),
        default_ttl: Duration::from_secs(60),
        max_entry_bytes: 1024,
        path_levels: vec![2],
        loader_batch_entries: 100,
        loader_sleep: Duration::ZERO,
        manager_batch_entries: 100,
        manager_sleep: Duration::ZERO,
        inactive_cleanup_interval: Duration::from_secs(60),
        shared_index: true,
    };
    let key = "https:example.com:/legacy-shared";
    let hash = cache_key_hash(key);
    let now = unix_time_ms(SystemTime::now());
    let legacy_path = zone.path.join(".rginx-index.json");
    let shared_db_path = zone.path.join(".rginx-index.sqlite3");

    std::fs::write(
        &legacy_path,
        serde_json::to_vec(&serde_json::json!({
            "version": 2,
            "generation": 7,
            "entries": [
                {
                    "key": key,
                    "hash": hash,
                    "base_key": key,
                    "stored_at_unix_ms": now,
                    "vary": [],
                    "tags": [],
                    "body_size_bytes": 6,
                    "expires_at_unix_ms": now.saturating_add(60_000),
                    "last_access_unix_ms": now
                }
            ],
            "admission_counts": [
                {
                    "key": key,
                    "uses": 3
                }
            ]
        }))
        .expect("legacy shared index should serialize"),
    )
    .expect("legacy shared index should be written");

    let (index, store, generation, store_epoch, change_seq) =
        shared::bootstrap_shared_index(&zone).expect("legacy bootstrap should load");
    assert!(store.is_some(), "shared metadata store should be initialized");
    assert_eq!(generation, 7);
    assert!(store_epoch > 0);
    assert_eq!(change_seq, 0);
    assert_eq!(index.admission_counts.get(key), Some(&3));
    let entry = index.entries.get(key).expect("legacy shared entry should import");
    assert_eq!(entry.hash, hash);
    assert_eq!(entry.base_key, key);
    assert_eq!(entry.body_size_bytes, 6);
    assert!(!legacy_path.exists(), "legacy sidecar should be removed after import");
    assert!(shared_db_path.exists(), "shared metadata db should be created");

    let (reloaded, reloaded_store, reloaded_generation, reloaded_store_epoch, reloaded_change_seq) =
        shared::bootstrap_shared_index(&zone).expect("sqlite bootstrap should load after import");
    assert!(reloaded_store.is_some(), "sqlite-backed store should remain available");
    assert_eq!(reloaded_generation, 7);
    assert_eq!(reloaded_store_epoch, store_epoch);
    assert_eq!(reloaded_change_seq, 0);
    assert_eq!(reloaded.admission_counts.get(key), Some(&3));
    assert_eq!(reloaded.entries.get(key).expect("sqlite-backed entry should reload").hash, hash);
}

#[test]
fn bootstrap_shared_index_skips_unreadable_legacy_sidecar_and_rebuilds_from_cache_files() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = CacheZone {
        name: "default".to_string(),
        path: temp.path().to_path_buf(),
        max_size_bytes: Some(1024 * 1024),
        inactive: Duration::from_secs(60),
        default_ttl: Duration::from_secs(60),
        max_entry_bytes: 1024,
        path_levels: vec![2],
        loader_batch_entries: 100,
        loader_sleep: Duration::ZERO,
        manager_batch_entries: 100,
        manager_sleep: Duration::ZERO,
        inactive_cleanup_interval: Duration::from_secs(60),
        shared_index: true,
    };
    let key = "https:example.com:/legacy-shared-corrupt";
    let hash = cache_key_hash(key);
    let now = unix_time_ms(SystemTime::now());
    let legacy_path = zone.path.join(".rginx-index.json");
    let shared_db_path = zone.path.join(".rginx-index.sqlite3");
    let paths = cache_paths_for_zone(&zone, &hash);

    std::fs::create_dir_all(&paths.dir).expect("cache dir should be created");
    std::fs::write(&legacy_path, b"{not-json").expect("corrupt legacy sidecar should be written");
    std::fs::write(
        &paths.metadata,
        serde_json::to_vec(&cache_metadata(
            key.to_string(),
            StatusCode::OK,
            &http::HeaderMap::new(),
            test_metadata_input(key, now, now.saturating_add(60_000), 6),
        ))
        .expect("cache metadata should serialize"),
    )
    .expect("cache metadata should be written");
    std::fs::write(&paths.body, b"cached").expect("cache body should be written");

    let (index, store, generation, store_epoch, change_seq) =
        shared::bootstrap_shared_index(&zone).expect("bootstrap should fall back to cache files");
    assert!(store.is_some(), "shared metadata store should still be initialized");
    assert_eq!(generation, 1);
    assert!(store_epoch > 0);
    assert_eq!(change_seq, 0);
    assert!(legacy_path.exists(), "corrupt legacy sidecar should be left in place");
    assert!(shared_db_path.exists(), "shared metadata db should be created");
    let entry = index.entries.get(key).expect("cache file entry should be loaded");
    assert_eq!(entry.hash, hash);
    assert_eq!(entry.body_size_bytes, 6);
}
