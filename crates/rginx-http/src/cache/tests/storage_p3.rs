use bytes::Bytes;
use http::header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, RANGE};
use http::{Method, Request, Response, StatusCode};
use http_body_util::BodyExt;
use std::time::{Duration, SystemTime};

use crate::handler::full_body;

use super::*;

fn test_manager_with_max_size(path: std::path::PathBuf, max_size_bytes: usize) -> CacheManager {
    let config = Arc::new(CacheZone {
        name: "default".to_string(),
        path,
        max_size_bytes: Some(max_size_bytes),
        inactive: Duration::from_secs(1),
        default_ttl: Duration::from_secs(60),
        max_entry_bytes: 1024,
        path_levels: vec![2],
        loader_batch_entries: 100,
        loader_sleep: Duration::ZERO,
        manager_batch_entries: 100,
        manager_sleep: Duration::ZERO,
        inactive_cleanup_interval: Duration::ZERO,
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
async fn sliced_range_requests_rewrite_upstream_range_and_trim_hits() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.range_requests = rginx_core::CacheRangeRequestPolicy::Cache;
    policy.slice_size_bytes = Some(8);

    let first_request = Request::builder()
        .method(Method::GET)
        .uri("/slice")
        .header("host", "example.com")
        .header(RANGE, "bytes=2-4")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let first_context =
        match manager.lookup(CacheRequest::from_request(&first_request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("slice request should miss before storing"),
        };

    let mut upstream_headers = first_request.headers().clone();
    first_context.apply_upstream_request_headers(&mut upstream_headers);
    assert_eq!(upstream_headers.get(RANGE).unwrap(), "bytes=0-7");

    let upstream_response = Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(CONTENT_RANGE, "bytes 0-7/26")
        .header(CONTENT_LENGTH, "8")
        .body(full_body("abcdefgh"))
        .expect("response should build");
    let first_stored = manager.store_response(first_context, upstream_response).await;
    assert_eq!(first_stored.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");
    assert_eq!(first_stored.headers().get(CONTENT_RANGE).unwrap(), "bytes 2-4/26");
    assert_eq!(first_stored.headers().get(CONTENT_LENGTH).unwrap(), "3");
    let first_body = first_stored.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(first_body.as_ref(), b"cde");

    let second_request = Request::builder()
        .method(Method::GET)
        .uri("/slice")
        .header("host", "example.com")
        .header(RANGE, "bytes=5-6")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    match manager.lookup(CacheRequest::from_request(&second_request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            assert_eq!(response.headers().get(CACHE_STATUS_HEADER).unwrap(), "HIT");
            assert_eq!(response.headers().get(CONTENT_RANGE).unwrap(), "bytes 5-6/26");
            assert_eq!(response.headers().get(CONTENT_LENGTH).unwrap(), "2");
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"fg");
        }
        _ => panic!("subrange within the same slice should hit the cached slice"),
    }
}

#[tokio::test]
async fn sliced_range_requests_fallback_to_passthrough_when_origin_ignores_range() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.range_requests = rginx_core::CacheRangeRequestPolicy::Cache;
    policy.slice_size_bytes = Some(8);

    let request = Request::builder()
        .method(Method::GET)
        .uri("/slice")
        .header("host", "example.com")
        .header(RANGE, "bytes=2-4")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let context = match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await
    {
        CacheLookup::Miss(context) => *context,
        _ => panic!("slice request should miss before storing"),
    };

    let upstream_response = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_LENGTH, "8")
        .body(full_body("abcdefgh"))
        .expect("response should build");
    let downstream = manager.store_response(context, upstream_response).await;
    assert_eq!(downstream.status(), StatusCode::OK);
    assert_eq!(downstream.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");
    assert!(downstream.headers().get(CONTENT_RANGE).is_none());
    let body = downstream.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"abcdefgh");

    assert!(matches!(
        manager.lookup(CacheRequest::from_request(&request), "https", &policy).await,
        CacheLookup::Miss(_)
    ));
}

#[tokio::test]
async fn inactive_cleanup_reschedules_hot_candidate_and_continues_to_next_due_entry() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager_with_max_size(temp.path().to_path_buf(), 1024 * 1024);
    let zone = manager.zones.get("default").expect("default zone should exist").clone();
    let policy = test_policy();
    let now = unix_time_ms(SystemTime::now());
    let hot_key = "https:example.com:/inactive-hot";
    let cold_key = "https:example.com:/inactive-cold";

    for (key, body, last_access_unix_ms) in [
        (hot_key, b"hot".as_slice(), now.saturating_sub(6_000)),
        (cold_key, b"old".as_slice(), now.saturating_sub(5_000)),
    ] {
        let hash = cache_key_hash(key);
        let metadata = cache_metadata(
            key.to_string(),
            StatusCode::OK,
            Response::builder()
                .status(StatusCode::OK)
                .header(CACHE_CONTROL, "max-age=60")
                .body(())
                .expect("metadata response should build")
                .headers(),
            test_metadata_input(key, last_access_unix_ms, now.saturating_add(60_000), body.len()),
        );
        let paths = cache_paths_for_zone(zone.config.as_ref(), &hash);
        write_cache_entry(&paths, &metadata, body).await.expect("entry should be written");
        let mut index = lock_index(&zone.index);
        index.insert_entry(
            key.to_string(),
            test_index_entry(
                key,
                hash,
                body.len(),
                now.saturating_add(60_000),
                last_access_unix_ms,
            ),
        );
        index.current_size_bytes = index.current_size_bytes.saturating_add(body.len());
    }

    let hot_request = Request::builder()
        .method(Method::GET)
        .uri("/inactive-hot")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    match manager.lookup(CacheRequest::from_request(&hot_request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"hot");
        }
        _ => panic!("hot candidate should hit before cleanup"),
    }

    cleanup_inactive_entries_in_zone(&zone).await;

    let index = lock_index(&zone.index);
    assert!(index.entries.contains_key(hot_key), "local-hot key should be rescheduled");
    assert!(!index.entries.contains_key(cold_key), "next due cold key should still be cleaned");
}

#[tokio::test]
async fn eviction_reschedules_local_hot_candidate_before_removing_older_entry() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager_with_max_size(temp.path().to_path_buf(), 6);
    let zone = manager.zones.get("default").expect("default zone should exist").clone();
    let policy = test_policy();
    let request_a = Request::builder()
        .method(Method::GET)
        .uri("/evict-a")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let request_b = Request::builder()
        .method(Method::GET)
        .uri("/evict-b")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let request_c = Request::builder()
        .method(Method::GET)
        .uri("/evict-c")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    let context_a =
        match manager.lookup(CacheRequest::from_request(&request_a), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("first key should miss before store"),
        };
    let _ = drain_response(
        manager
            .store_response(
                context_a,
                Response::builder()
                    .status(StatusCode::OK)
                    .header(CACHE_CONTROL, "max-age=60")
                    .body(full_body("aaa"))
                    .expect("response should build"),
            )
            .await,
    )
    .await;
    let _ = wait_for_hit(&manager, &request_a, &policy).await;

    let context_b =
        match manager.lookup(CacheRequest::from_request(&request_b), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("second key should miss before store"),
        };
    let _ = drain_response(
        manager
            .store_response(
                context_b,
                Response::builder()
                    .status(StatusCode::OK)
                    .header(CACHE_CONTROL, "max-age=60")
                    .body(full_body("bbb"))
                    .expect("response should build"),
            )
            .await,
    )
    .await;
    let _ = wait_for_hit(&manager, &request_b, &policy).await;

    {
        let index = lock_index(&zone.index);
        assert_eq!(
            index.access_schedule.len(),
            2,
            "unexpected schedule after storing A/B: entries={:?} schedule={:?}",
            index.entries.keys().collect::<Vec<_>>(),
            index
                .access_schedule
                .iter()
                .map(|entry| (&entry.key, entry.last_access_unix_ms))
                .collect::<Vec<_>>()
        );
    }
    let reorder_now = unix_time_ms(SystemTime::now());
    {
        let mut index = lock_index(&zone.index);
        index.entries.get_mut("https:example.com:/evict-a").unwrap().last_access_unix_ms =
            reorder_now.saturating_sub(20);
        index.entries.get_mut("https:example.com:/evict-b").unwrap().last_access_unix_ms =
            reorder_now.saturating_sub(10);
        index.rebuild_access_schedule();
    }
    zone.clear_hot_entries();

    match manager.lookup(CacheRequest::from_request(&request_a), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"aaa");
        }
        _ => panic!("first key should hit before eviction"),
    }

    let context_c =
        match manager.lookup(CacheRequest::from_request(&request_c), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("third key should miss before store"),
        };
    let _ = drain_response(
        manager
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
    let _ = wait_for_hit(&manager, &request_c, &policy).await;

    let index = lock_index(&zone.index);
    assert!(index.entries.contains_key("https:example.com:/evict-a"));
    assert!(
        !index.entries.contains_key("https:example.com:/evict-b"),
        "unexpected keys after eviction: {:?}",
        index.entries.keys().collect::<Vec<_>>()
    );
    assert!(index.entries.contains_key("https:example.com:/evict-c"));
}
