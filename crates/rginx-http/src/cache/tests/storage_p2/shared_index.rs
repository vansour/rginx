use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use http::header::CACHE_CONTROL;
use http::{Method, Request, Response, StatusCode};
use http_body_util::BodyExt;

use crate::handler::full_body;

use super::*;

mod sync_regressions;

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
