use std::time::SystemTime;

use bytes::Bytes;
use futures_util::stream;
use http::header::CACHE_CONTROL;
use http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, StreamBody};
use hyper::body::Frame;
use tokio::time::timeout;

use crate::handler::{BoxError, boxed_body, full_body};

use super::*;

#[cfg(target_os = "linux")]
fn shared_memory_test_config(path: std::path::PathBuf) -> Arc<CacheZone> {
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

#[cfg(target_os = "linux")]
fn shared_memory_test_zone(config: Arc<CacheZone>) -> Arc<CacheZoneRuntime> {
    let (
        index,
        shared_index_store,
        shared_index_generation,
        shared_index_store_epoch,
        shared_index_change_seq,
    ) = shared::bootstrap_shared_index(config.as_ref())
        .expect("test shared index should bootstrap");
    Arc::new(CacheZoneRuntime {
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
    })
}

#[cfg(target_os = "linux")]
fn shared_memory_test_zone_pair(
    path: std::path::PathBuf,
) -> (Arc<CacheZone>, Arc<CacheZoneRuntime>, Arc<CacheZoneRuntime>) {
    let config = shared_memory_test_config(path);
    let _ = shared::unlink_memory_shared_index_for_test(config.as_ref());
    let zone_a = shared_memory_test_zone(config.clone());
    let zone_b = shared_memory_test_zone(config.clone());
    (config, zone_a, zone_b)
}

#[cfg(target_os = "linux")]
fn shared_memory_test_manager_pair(
    path: std::path::PathBuf,
) -> (Arc<CacheZone>, CacheManager, CacheManager) {
    let (config, zone_a, zone_b) = shared_memory_test_zone_pair(path);
    (
        config,
        CacheManager { zones: Arc::new(HashMap::from([("default".to_string(), zone_a)])) },
        CacheManager { zones: Arc::new(HashMap::from([("default".to_string(), zone_b)])) },
    )
}

#[test]
fn shared_fill_locks_coordinate_across_zone_instances() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone_a = test_zone(temp.path().to_path_buf(), 1024);
    let zone_b = test_zone(temp.path().to_path_buf(), 1024);
    let now = unix_time_ms(SystemTime::now());

    let guard = match zone_a.fill_lock_decision("shared-key", now, Duration::from_secs(5), None) {
        FillLockDecision::Acquired(guard) => guard,
        _ => panic!("first zone should acquire the shared fill lock"),
    };

    match zone_b.fill_lock_decision("shared-key", now, Duration::from_secs(5), None) {
        FillLockDecision::WaitExternal { key } => assert_eq!(key, "shared-key"),
        _ => panic!("second zone should wait on the external shared fill lock"),
    }

    drop(guard);

    match zone_b.fill_lock_decision("shared-key", now, Duration::from_secs(5), None) {
        FillLockDecision::Acquired(_) => {}
        _ => panic!("second zone should acquire the shared fill lock after release"),
    }
}

#[cfg(target_os = "linux")]
#[test]
fn shared_memory_fill_locks_coordinate_without_lock_files() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let (config, zone_a, zone_b) = shared_memory_test_zone_pair(temp.path().to_path_buf());
    let key = "shared-memory-key";
    let lock_path = shared::shared_fill_lock_path(config.as_ref(), key);
    let state_path = shared::shared_fill_state_path(config.as_ref(), key);
    let now = unix_time_ms(SystemTime::now());

    let guard = match zone_a.fill_lock_decision(key, now, Duration::from_secs(5), None) {
        FillLockDecision::Acquired(guard) => guard,
        _ => panic!("first zone should acquire the shm shared fill lock"),
    };
    assert!(guard.external_lock_path.is_none(), "shm fill lock should not retain a file lock path");
    assert!(!lock_path.exists(), "shm fill lock should not create a lock file");
    assert!(!state_path.exists(), "shm fill lock should not create a state file");

    match zone_b.fill_lock_decision(key, now, Duration::from_secs(5), None) {
        FillLockDecision::WaitExternal { key: waited_key } => assert_eq!(waited_key, key),
        _ => panic!("second zone should wait on the shm shared fill lock"),
    }

    drop(guard);

    match zone_b.fill_lock_decision(key, now, Duration::from_secs(5), None) {
        FillLockDecision::Acquired(guard) => {
            assert!(guard.external_lock_path.is_none());
            drop(guard);
        }
        _ => panic!("second zone should acquire the shm shared fill lock after release"),
    }
    assert!(!lock_path.exists(), "shm fill lock should not create a lock file");
    assert!(!state_path.exists(), "shm fill lock should not create a state file");

    shared::unlink_memory_shared_index_for_test(config.as_ref())
        .expect("test shm segment should unlink");
}

#[cfg(target_os = "linux")]
#[test]
fn shared_memory_fill_lock_stale_record_can_be_reclaimed() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let (config, zone_a, zone_b) = shared_memory_test_zone_pair(temp.path().to_path_buf());
    let key = "stale-shared-memory-key";
    let old_now = unix_time_ms(SystemTime::now()).saturating_sub(5_000);
    let guard = match zone_a.fill_lock_decision(key, old_now, Duration::from_secs(5), None) {
        FillLockDecision::Acquired(guard) => guard,
        _ => panic!("first zone should acquire the old shm fill lock"),
    };

    match zone_b.fill_lock_decision(
        key,
        unix_time_ms(SystemTime::now()),
        Duration::from_millis(1),
        None,
    ) {
        FillLockDecision::Acquired(new_guard) => {
            assert!(new_guard.external_lock_path.is_none());
            drop(new_guard);
        }
        _ => panic!("stale shm fill lock should be reclaimable by another zone"),
    }
    assert!(zone_b.snapshot().shared_index_stale_fill_lock_cleanup_total >= 1);
    drop(guard);

    shared::unlink_memory_shared_index_for_test(config.as_ref())
        .expect("test shm segment should unlink");
}

#[test]
fn shared_fill_state_init_failure_falls_back_to_local_fill_lock() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path().to_path_buf(), 1024);
    let key = "shared-key";
    let state_path = super::super::shared::shared_fill_state_path(zone.config.as_ref(), key);
    std::fs::create_dir(&state_path).expect("state path blocker should be creatable");

    let now = unix_time_ms(SystemTime::now());
    let guard = match zone.fill_lock_decision(key, now, Duration::from_secs(5), None) {
        FillLockDecision::Acquired(guard) => guard,
        _ => panic!("shared fill init failure should fall back to a local fill lock"),
    };
    assert!(
        guard.external_lock_path.is_none(),
        "fallback lock should not retain a broken external coordination file"
    );

    match zone.fill_lock_decision(key, now, Duration::from_secs(5), None) {
        FillLockDecision::WaitLocal { .. } => {}
        _ => panic!(
            "subsequent lookups should wait on the local fill lock, not spin on external coordination"
        ),
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn shared_memory_fill_locks_stream_from_external_inflight_fill_without_state_files() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let (config, manager_a, manager_b) = shared_memory_test_manager_pair(temp.path().to_path_buf());
    let policy = test_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/shared-memory-stream")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let key = "https:example.com:/shared-memory-stream";
    let lock_path = shared::shared_fill_lock_path(config.as_ref(), key);
    let state_path = shared::shared_fill_state_path(config.as_ref(), key);

    let context =
        match manager_a.lookup(CacheRequest::from_request(&request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("first manager should miss before shm shared stream fill"),
        };

    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let stream =
        stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|frame| (frame, rx)) });
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(boxed_body(StreamBody::new(stream)))
        .expect("streaming response should build");
    let owner = timeout(Duration::from_millis(200), manager_a.store_response(context, response))
        .await
        .expect("shm shared streaming cache store should start immediately");
    assert_eq!(owner.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");
    drop(owner);
    assert!(!lock_path.exists(), "shm fill should not create a lock file");
    assert!(!state_path.exists(), "shm fill should not create a state file");

    tx.send(Ok::<Frame<Bytes>, BoxError>(Frame::data(Bytes::from_static(b"chunk-1"))))
        .await
        .expect("first shared stream chunk should send");

    let shared =
        match manager_b.lookup(CacheRequest::from_request(&request), "https", &policy).await {
            CacheLookup::Hit(response) => response,
            CacheLookup::Miss(_) => panic!("second manager should reuse the shm in-flight fill"),
            CacheLookup::Updating(_, _) => {
                panic!("second manager should stream from the shm in-flight fill")
            }
            CacheLookup::Bypass(status) => {
                panic!("shm shared stream request should not bypass cache: {status:?}")
            }
        };
    assert_eq!(shared.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");
    let mut shared_body = shared.into_body();
    let first_frame = timeout(Duration::from_millis(200), shared_body.frame())
        .await
        .expect("external shm fill frame should arrive")
        .expect("external shm fill body should yield a frame")
        .expect("external shm fill frame should read");
    assert_eq!(first_frame.data_ref().unwrap().as_ref(), b"chunk-1");

    tx.send(Ok::<Frame<Bytes>, BoxError>(Frame::data(Bytes::from_static(b"chunk-2"))))
        .await
        .expect("second shared stream chunk should send");
    drop(tx);

    let remaining = shared_body.collect().await.unwrap().to_bytes();
    assert_eq!(
        [first_frame.data_ref().unwrap().as_ref(), remaining.as_ref()].concat(),
        b"chunk-1chunk-2"
    );
    assert!(!lock_path.exists(), "shm fill should not create a lock file");
    assert!(!state_path.exists(), "shm fill should not create a state file");

    let response = wait_for_hit(&manager_b, &request, &policy).await;
    assert_eq!(response.headers().get(CACHE_STATUS_HEADER).unwrap(), "HIT");
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"chunk-1chunk-2");

    shared::unlink_memory_shared_index_for_test(config.as_ref())
        .expect("test shm segment should unlink");
}

#[tokio::test]
async fn shared_fill_locks_stream_from_external_inflight_fill() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager_a = test_manager(temp.path().to_path_buf(), 1024);
    let manager_b = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/shared-stream")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    let context =
        match manager_a.lookup(CacheRequest::from_request(&request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("first manager should miss before shared stream fill"),
        };

    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let stream =
        stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|frame| (frame, rx)) });
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(boxed_body(StreamBody::new(stream)))
        .expect("streaming response should build");
    let owner = timeout(Duration::from_millis(200), manager_a.store_response(context, response))
        .await
        .expect("shared streaming cache store should start immediately");
    assert_eq!(owner.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");
    drop(owner);

    tx.send(Ok::<Frame<Bytes>, BoxError>(Frame::data(Bytes::from_static(b"chunk-1"))))
        .await
        .expect("first shared stream chunk should send");

    let shared = match manager_b
        .lookup(CacheRequest::from_request(&request), "https", &policy)
        .await
    {
        CacheLookup::Hit(response) => response,
        CacheLookup::Miss(_) => panic!("second manager should reuse the external in-flight fill"),
        CacheLookup::Updating(_, _) => {
            panic!("second manager should stream from the external in-flight fill")
        }
        CacheLookup::Bypass(status) => {
            panic!("shared stream request should not bypass cache: {status:?}")
        }
    };
    assert_eq!(shared.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");
    let mut shared_body = shared.into_body();
    let first_frame = timeout(Duration::from_millis(200), shared_body.frame())
        .await
        .expect("external shared fill frame should arrive")
        .expect("external shared fill body should yield a frame")
        .expect("external shared fill frame should read");
    assert_eq!(first_frame.data_ref().unwrap().as_ref(), b"chunk-1");

    tx.send(Ok::<Frame<Bytes>, BoxError>(Frame::data(Bytes::from_static(b"chunk-2"))))
        .await
        .expect("second shared stream chunk should send");
    drop(tx);

    let remaining = shared_body.collect().await.unwrap().to_bytes();
    assert_eq!(
        [first_frame.data_ref().unwrap().as_ref(), remaining.as_ref()].concat(),
        b"chunk-1chunk-2"
    );

    let response = wait_for_hit(&manager_b, &request, &policy).await;
    assert_eq!(response.headers().get(CACHE_STATUS_HEADER).unwrap(), "HIT");
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"chunk-1chunk-2");
}

#[tokio::test]
async fn head_requests_can_populate_get_cache_entries_when_convert_head_is_enabled() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.methods = vec![Method::GET];
    policy.convert_head = true;

    let head_request = Request::builder()
        .method(Method::HEAD)
        .uri("/head-fill")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let context =
        match manager.lookup(CacheRequest::from_request(&head_request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            CacheLookup::Bypass(status) => {
                panic!("convert_head should allow HEAD to use GET cache methods: {status:?}")
            }
            _ => panic!("empty cache should miss for HEAD fill"),
        };
    assert_eq!(context.upstream_request_method(), Method::GET);

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("from get upstream"))
        .expect("response should build");
    let _ = drain_response(manager.store_response(context, response).await).await;

    let get_request = Request::builder()
        .method(Method::GET)
        .uri("/head-fill")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    match manager.lookup(CacheRequest::from_request(&get_request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"from get upstream");
        }
        _ => panic!("HEAD fill should populate a GET-cacheable entry"),
    }
}
