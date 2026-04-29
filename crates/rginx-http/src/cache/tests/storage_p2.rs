use std::sync::atomic::Ordering;
use std::time::SystemTime;

use bytes::Bytes;
use http::header::CACHE_CONTROL;
use http::{Method, Request, Response, StatusCode};
use http_body_util::BodyExt;

use crate::handler::full_body;

use super::*;

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
    let _ = manager_a.store_response(context_a, response_a).await;

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
    let _ = manager_b.store_response(context_b, response_b).await;

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
    let _ = manager_a.store_response(context, response).await;

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
    let _ = manager_a.store_response(context, response).await;

    match manager_b.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Hit(_) => {}
        _ => panic!("second manager should hit the shared cache entry"),
    }

    let purge_key = "https:example.com:/purged-shared";
    let purge = manager_a.purge_key("default", purge_key).await.expect("purge should succeed");
    assert_eq!(purge.removed_entries, 1);

    match manager_b.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(_) => {}
        _ => panic!("purged shared entry should no longer hit on the second manager"),
    }
}

#[test]
fn bootstrap_shared_index_imports_legacy_sidecar_into_shared_metadata_db() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path().to_path_buf(), 1024);
    let key = "https:example.com:/legacy-shared";
    let hash = cache_key_hash(key);
    let now = unix_time_ms(SystemTime::now());
    let legacy_path = zone.config.path.join(".rginx-index.json");
    let shared_db_path = zone.config.path.join(".rginx-index.sqlite3");

    std::fs::write(
        &legacy_path,
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "generation": 7,
            "entries": [
                {
                    "key": key,
                    "hash": hash,
                    "base_key": key,
                    "vary": [],
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

    let (index, store, generation) =
        shared::bootstrap_shared_index(zone.config.as_ref()).expect("legacy bootstrap should load");
    assert!(store.is_some(), "shared metadata store should be initialized");
    assert_eq!(generation, 7);
    assert_eq!(index.admission_counts.get(key), Some(&3));
    let entry = index.entries.get(key).expect("legacy shared entry should import");
    assert_eq!(entry.hash, hash);
    assert_eq!(entry.base_key, key);
    assert_eq!(entry.body_size_bytes, 6);
    assert!(!legacy_path.exists(), "legacy sidecar should be removed after import");
    assert!(shared_db_path.exists(), "shared metadata db should be created");

    let (reloaded, reloaded_store, reloaded_generation) =
        shared::bootstrap_shared_index(zone.config.as_ref())
            .expect("sqlite bootstrap should load after import");
    assert!(reloaded_store.is_some(), "sqlite-backed store should remain available");
    assert_eq!(reloaded_generation, 7);
    assert_eq!(reloaded.admission_counts.get(key), Some(&3));
    assert_eq!(reloaded.entries.get(key).expect("sqlite-backed entry should reload").hash, hash);
}

#[test]
fn bootstrap_shared_index_skips_unreadable_legacy_sidecar_and_rebuilds_from_cache_files() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path().to_path_buf(), 1024);
    let key = "https:example.com:/legacy-shared-corrupt";
    let hash = cache_key_hash(key);
    let now = unix_time_ms(SystemTime::now());
    let legacy_path = zone.config.path.join(".rginx-index.json");
    let shared_db_path = zone.config.path.join(".rginx-index.sqlite3");
    let paths = cache_paths_for_zone(zone.config.as_ref(), &hash);

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

    let (index, store, generation) = shared::bootstrap_shared_index(zone.config.as_ref())
        .expect("bootstrap should fall back to cache files");
    assert!(store.is_some(), "shared metadata store should still be initialized");
    assert_eq!(generation, 1);
    assert!(legacy_path.exists(), "corrupt legacy sidecar should be left in place");
    assert!(shared_db_path.exists(), "shared metadata db should be created");
    let entry = index.entries.get(key).expect("cache file entry should be loaded");
    assert_eq!(entry.hash, hash);
    assert_eq!(entry.body_size_bytes, 6);
}

#[test]
fn shared_fill_locks_coordinate_across_zone_instances() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone_a = test_zone(temp.path().to_path_buf(), 1024);
    let zone_b = test_zone(temp.path().to_path_buf(), 1024);
    let now = unix_time_ms(SystemTime::now());

    let guard = match zone_a.fill_lock_decision("shared-key", now, Duration::from_secs(5)) {
        FillLockDecision::Acquired(guard) => guard,
        _ => panic!("first zone should acquire the shared fill lock"),
    };

    match zone_b.fill_lock_decision("shared-key", now, Duration::from_secs(5)) {
        FillLockDecision::WaitExternal { key } => assert_eq!(key, "shared-key"),
        _ => panic!("second zone should wait on the external shared fill lock"),
    }

    drop(guard);

    match zone_b.fill_lock_decision("shared-key", now, Duration::from_secs(5)) {
        FillLockDecision::Acquired(_) => {}
        _ => panic!("second zone should acquire the shared fill lock after release"),
    }
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
    let _ = manager.store_response(context, response).await;

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
