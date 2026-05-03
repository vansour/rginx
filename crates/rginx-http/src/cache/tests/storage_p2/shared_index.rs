use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use http::header::CACHE_CONTROL;
use http::{Method, Request, Response, StatusCode};
use http_body_util::BodyExt;
use tokio::task::JoinSet;

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

fn test_manager_with_shared_config(config: Arc<CacheZone>) -> CacheManager {
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

fn test_shared_backend_config(path: std::path::PathBuf) -> Arc<CacheZone> {
    Arc::new(CacheZone {
        name: "default".to_string(),
        path,
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
    })
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

#[cfg(target_os = "linux")]
#[tokio::test]
async fn shared_memory_index_backend_shares_entries_between_managers() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let config = test_shared_backend_config(temp.path().to_path_buf());
    let _ = shared::unlink_memory_shared_index_for_test(config.as_ref());
    let manager_a = test_manager_with_shared_config(config.clone());
    let manager_b = test_manager_with_shared_config(config.clone());
    let policy = test_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/shared-memory")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    let context =
        match manager_a.lookup(CacheRequest::from_request(&request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("empty shm-backed cache should miss"),
        };
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("shared-memory"))
        .expect("response should build");
    let _ = drain_response(manager_a.store_response(context, response).await).await;

    match manager_b.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            assert_eq!(response.headers().get(CACHE_STATUS_HEADER).unwrap(), "HIT");
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"shared-memory");
        }
        CacheLookup::Miss(_) => panic!("second manager should sync the shm shared index and hit"),
        CacheLookup::Updating(_, _) => panic!("fresh shm shared entry should not update"),
        CacheLookup::Bypass(status) => {
            panic!("shm shared cache request should not bypass: {status:?}")
        }
    }

    let purge_key = "https:example.com:/shared-memory";
    let purge = manager_a.purge_key("default", purge_key).await.expect("purge should succeed");
    assert_eq!(purge.removed_entries, 1);
    match manager_b.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(_) => {}
        _ => panic!("purged shm shared entry should no longer hit on the second manager"),
    }

    drop(manager_a);
    drop(manager_b);
    shared::unlink_memory_shared_index_for_test(config.as_ref())
        .expect("test shm segment should unlink");
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_memory_index_handles_parallel_hit_touches() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let config = test_shared_backend_config(temp.path().to_path_buf());
    let _ = shared::unlink_memory_shared_index_for_test(config.as_ref());
    let manager_a = test_manager_with_shared_config(config.clone());
    let manager_b = test_manager_with_shared_config(config.clone());
    let policy = Arc::new(test_policy());
    let request = Request::builder()
        .method(Method::GET)
        .uri("/shared-memory-parallel-hit")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    let context =
        match manager_a.lookup(CacheRequest::from_request(&request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("empty shm-backed cache should miss"),
        };
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("parallel-hit"))
        .expect("response should build");
    let _ = drain_response(manager_a.store_response(context, response).await).await;
    let _ = wait_for_hit(&manager_a, &request, &policy).await;
    let _ = wait_for_hit(&manager_b, &request, &policy).await;

    let mut tasks = JoinSet::new();
    for task_index in 0..32 {
        let manager = if task_index % 2 == 0 { manager_a.clone() } else { manager_b.clone() };
        let policy = policy.clone();
        tasks.spawn(async move {
            let request = Request::builder()
                .method(Method::GET)
                .uri("/shared-memory-parallel-hit")
                .header("host", "example.com")
                .body(full_body(Bytes::new()))
                .expect("request should build");
            match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
                CacheLookup::Hit(response) => {
                    assert_eq!(response.headers().get(CACHE_STATUS_HEADER).unwrap(), "HIT");
                    let body = response.into_body().collect().await.unwrap().to_bytes();
                    assert_eq!(body.as_ref(), b"parallel-hit");
                }
                CacheLookup::Miss(_) => panic!("parallel shared memory hit should not miss"),
                CacheLookup::Updating(_, _) => {
                    panic!("parallel shared memory hit should not update")
                }
                CacheLookup::Bypass(status) => {
                    panic!("parallel shared memory hit should not bypass: {status:?}")
                }
            }
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.expect("parallel shared memory hit task should finish");
    }

    let snapshot = manager_a.snapshot_with_shared_sync().await;
    assert_eq!(snapshot[0].shared_index_entry_count, 1);
    assert!(snapshot[0].shared_index_shm_used_bytes > 0);
    assert!(snapshot[0].shared_index_operation_ring_used > 0);

    drop(manager_a);
    drop(manager_b);
    shared::unlink_memory_shared_index_for_test(config.as_ref())
        .expect("test shm segment should unlink");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn shared_memory_index_backend_shares_admission_counts_between_managers() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let config = test_shared_backend_config(temp.path().to_path_buf());
    let _ = shared::unlink_memory_shared_index_for_test(config.as_ref());
    let manager_a = test_manager_with_shared_config(config.clone());
    let manager_b = test_manager_with_shared_config(config.clone());
    let mut policy = test_policy();
    policy.min_uses = 2;
    let request = Request::builder()
        .method(Method::GET)
        .uri("/shared-memory-min-uses")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    let context_a =
        match manager_a.lookup(CacheRequest::from_request(&request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("first shm-backed manager should miss"),
        };
    let response_a = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("shared-memory-min-uses"))
        .expect("response should build");
    let _ = drain_response(manager_a.store_response(context_a, response_a).await).await;

    let context_b =
        match manager_b.lookup(CacheRequest::from_request(&request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("second shm-backed manager should miss before the second admission"),
        };
    let response_b = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("shared-memory-min-uses"))
        .expect("response should build");
    let _ = drain_response(manager_b.store_response(context_b, response_b).await).await;

    match manager_a.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"shared-memory-min-uses");
        }
        _ => {
            panic!("shared admission counts should allow the shm-backed second manager to populate")
        }
    }

    drop(manager_a);
    drop(manager_b);
    shared::unlink_memory_shared_index_for_test(config.as_ref())
        .expect("test shm segment should unlink");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn shared_memory_index_backend_propagates_tag_invalidations_between_managers() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let config = test_shared_backend_config(temp.path().to_path_buf());
    let _ = shared::unlink_memory_shared_index_for_test(config.as_ref());
    let manager_a = test_manager_with_shared_config(config.clone());
    let manager_b = test_manager_with_shared_config(config.clone());
    let policy = test_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/shared-memory-tagged")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    let context =
        match manager_a.lookup(CacheRequest::from_request(&request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("empty shm-backed cache should miss"),
        };
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .header("cache-tag", "news")
        .body(full_body("shared-memory-tagged"))
        .expect("response should build");
    let _ = drain_response(manager_a.store_response(context, response).await).await;

    match manager_b.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"shared-memory-tagged");
        }
        _ => panic!("second manager should hit shared shm entry before invalidation"),
    }

    let zone_b = manager_b.zones.get("default").expect("default zone should exist");
    let key = "https:example.com:/shared-memory-tagged";
    let hash = cache_key_hash(key);
    assert!(zone_b.prepared_response_head(key, &hash).is_some());

    let invalidation =
        manager_a.invalidate_tag("default", "news").await.expect("tag invalidation should work");
    assert_eq!(invalidation.affected_entries, 1);

    match manager_b.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(_) => {}
        CacheLookup::Hit(_) => panic!("shared shm invalidation must stop serving the entry"),
        CacheLookup::Updating(_, _) => {
            panic!("shared shm invalidation must not trigger background update")
        }
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }
    assert!(zone_b.prepared_response_head(key, &hash).is_none());

    drop(manager_a);
    drop(manager_b);
    shared::unlink_memory_shared_index_for_test(config.as_ref())
        .expect("test shm segment should unlink");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn shared_memory_index_backend_delta_sync_preserves_hot_heads_for_unchanged_keys() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let config = test_shared_backend_config(temp.path().to_path_buf());
    let _ = shared::unlink_memory_shared_index_for_test(config.as_ref());
    let manager_a = test_manager_with_shared_config(config.clone());
    let manager_b = test_manager_with_shared_config(config.clone());
    let policy = test_policy();
    let primary_request = Request::builder()
        .method(Method::GET)
        .uri("/shared-memory-delta-primary")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let secondary_request = Request::builder()
        .method(Method::GET)
        .uri("/shared-memory-delta-secondary")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    let primary_context = match manager_a
        .lookup(CacheRequest::from_request(&primary_request), "https", &policy)
        .await
    {
        CacheLookup::Miss(context) => *context,
        _ => panic!("first manager should miss before storing primary shm entry"),
    };
    let primary_response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("primary-shm"))
        .expect("response should build");
    let _ = drain_response(manager_a.store_response(primary_context, primary_response).await).await;

    match manager_b.lookup(CacheRequest::from_request(&primary_request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"primary-shm");
        }
        _ => panic!("second manager should hit the primary shm entry"),
    }

    let zone_b = manager_b.zones.get("default").expect("default zone should exist");
    let primary_key = "https:example.com:/shared-memory-delta-primary";
    let primary_hash = cache_key_hash(primary_key);
    assert!(zone_b.prepared_response_head(primary_key, &primary_hash).is_some());

    let secondary_context = match manager_a
        .lookup(CacheRequest::from_request(&secondary_request), "https", &policy)
        .await
    {
        CacheLookup::Miss(context) => *context,
        _ => panic!("first manager should miss before storing secondary shm entry"),
    };
    let secondary_response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("secondary-shm"))
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
            assert_eq!(body.as_ref(), b"primary-shm");
        }
        _ => panic!("shm delta sync should preserve unchanged hot response heads"),
    }

    assert!(zone_b.prepared_response_head(primary_key, &primary_hash).is_some());
    assert!(
        lock_index(&zone_b.index)
            .entries
            .contains_key("https:example.com:/shared-memory-delta-secondary")
    );

    drop(manager_a);
    drop(manager_b);
    shared::unlink_memory_shared_index_for_test(config.as_ref())
        .expect("test shm segment should unlink");
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
#[cfg(target_os = "linux")]
async fn shared_index_sync_keeps_local_hits_when_shared_memory_document_is_corrupted() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let config = test_shared_backend_config(temp.path().to_path_buf());
    let _ = shared::unlink_memory_shared_index_for_test(config.as_ref());
    let manager_a = test_manager_with_shared_config(config.clone());
    let manager_b = test_manager_with_shared_config(config.clone());
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
    shared::corrupt_memory_shared_index_document_for_test(config.as_ref())
        .expect("shared memory document should be corrupted");

    match manager_b.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"shared");
        }
        _ => panic!("corrupt shared metadata must not evict an already-synced local hit"),
    }

    assert_eq!(zone.shared_index_generation.load(Ordering::Relaxed), shared_generation);
    assert!(lock_index(&zone.index).entries.contains_key("https:example.com:/shared-corrupt"));

    drop(manager_a);
    drop(manager_b);
    shared::unlink_memory_shared_index_for_test(config.as_ref())
        .expect("test shm segment should unlink");
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
