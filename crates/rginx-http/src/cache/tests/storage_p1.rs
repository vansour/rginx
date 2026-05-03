use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;
use http::header::{ACCEPT_LANGUAGE, CACHE_CONTROL, CONTENT_RANGE, SET_COOKIE};
use http::{Method, Request, Response, StatusCode};
use http_body_util::BodyExt;
use tokio::sync::Mutex as AsyncMutex;

use crate::handler::full_body;

use super::*;

mod edge_cases;

#[tokio::test]
async fn cache_manager_requires_min_uses_before_storing() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.min_uses = 2;
    let request = Request::builder()
        .method(Method::GET)
        .uri("/min-uses")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    let first_context =
        match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("first request should miss"),
        };
    let first_response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("first"))
        .expect("response should build");
    let _ = drain_response(manager.store_response(first_context, first_response).await).await;

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(context) => {
            let second_response = Response::builder()
                .status(StatusCode::OK)
                .header(CACHE_CONTROL, "max-age=60")
                .body(full_body("second"))
                .expect("response should build");
            let _ = drain_response(manager.store_response(*context, second_response).await).await;
        }
        _ => panic!("second request should still miss before admission threshold is met"),
    }

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"second");
        }
        _ => panic!("response should be cached after min_uses is reached"),
    }
}

#[tokio::test]
async fn cache_manager_can_ignore_cache_control_set_cookie_and_vary() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.ignore_headers = vec![
        rginx_core::CacheIgnoreHeader::CacheControl,
        rginx_core::CacheIgnoreHeader::SetCookie,
        rginx_core::CacheIgnoreHeader::Vary,
    ];
    let request = Request::builder()
        .method(Method::GET)
        .uri("/ignore-headers")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let context = match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await
    {
        CacheLookup::Miss(context) => *context,
        _ => panic!("request should miss before storing"),
    };

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "no-store")
        .header(SET_COOKIE, "sid=1")
        .header("vary", "*")
        .body(full_body("shared"))
        .expect("response should build");
    let _ = drain_response(manager.store_response(context, response).await).await;

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"shared");
        }
        _ => panic!("ignored cache headers should not block storage"),
    }
}

#[tokio::test]
async fn cache_manager_can_ignore_x_accel_expires() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.ignore_headers = vec![rginx_core::CacheIgnoreHeader::XAccelExpires];
    let request = Request::builder()
        .method(Method::GET)
        .uri("/ignore-x-accel")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let context = match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await
    {
        CacheLookup::Miss(context) => *context,
        _ => panic!("request should miss before storing"),
    };

    let response = Response::builder()
        .status(StatusCode::OK)
        .header("x-accel-expires", "0")
        .body(full_body("cached anyway"))
        .expect("response should build");
    let _ = drain_response(manager.store_response(context, response).await).await;

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"cached anyway");
        }
        _ => panic!("ignored x-accel-expires should fall back to default ttl"),
    }
}

#[tokio::test]
async fn cache_manager_caches_configured_single_range_response() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.range_requests = rginx_core::CacheRangeRequestPolicy::Cache;
    let range_request = Request::builder()
        .method(Method::GET)
        .uri("/range")
        .header("host", "example.com")
        .header(http::header::RANGE, "bytes=0-3")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let context =
        match manager.lookup(CacheRequest::from_request(&range_request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("range request should miss before storing"),
        };

    let response = Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(CONTENT_RANGE, "bytes 0-3/10")
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("data"))
        .expect("response should build");
    let _ = drain_response(manager.store_response(context, response).await).await;

    match manager.lookup(CacheRequest::from_request(&range_request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"data");
        }
        _ => panic!("same range request should hit cached 206 response"),
    }

    let full_request = Request::builder()
        .method(Method::GET)
        .uri("/range")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    match manager.lookup(CacheRequest::from_request(&full_request), "https", &policy).await {
        CacheLookup::Miss(_) => {}
        _ => panic!("full request must not reuse range-specific cache entry"),
    }
}

#[test]
fn load_index_from_disk_supports_nested_path_levels() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = CacheZone {
        name: "default".to_string(),
        path: temp.path().to_path_buf(),
        max_size_bytes: Some(1024 * 1024),
        inactive: Duration::from_secs(60),
        default_ttl: Duration::from_secs(60),
        max_entry_bytes: 1024,
        path_levels: vec![1, 2],
        loader_batch_entries: 100,
        loader_sleep: Duration::ZERO,
        manager_batch_entries: 100,
        manager_sleep: Duration::ZERO,
        inactive_cleanup_interval: Duration::from_secs(60),
        shared_index: true,
    };
    let key = "https:example.com:/nested";
    let hash = cache_key_hash(key);
    let paths = cache_paths_for_zone(&zone, &hash);
    let now = unix_time_ms(SystemTime::now());
    std::fs::create_dir_all(&paths.dir).expect("cache dir should be created");
    std::fs::write(
        &paths.metadata,
        serde_json::to_vec(&cache_metadata(
            key.to_string(),
            StatusCode::OK,
            &http::HeaderMap::new(),
            test_metadata_input(key, now.saturating_sub(2_000), now.saturating_add(60_000), 6),
        ))
        .expect("metadata should serialize"),
    )
    .expect("metadata should be written");
    std::fs::write(&paths.body, b"cached").expect("body should be written");

    let index = load_index_from_disk(&zone).expect("index should load");

    assert!(index.entries.contains_key(key));
    assert!(paths.metadata.exists());
    assert!(paths.body.exists());
}
