use std::time::SystemTime;

use bytes::Bytes;
use futures_util::stream;
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{Request, Response, StatusCode};
use http_body_util::{BodyExt, StreamBody};
use hyper::body::Frame;

use crate::handler::{BoxError, boxed_body, full_body};

use super::*;

#[tokio::test]
async fn cache_manager_returns_miss_then_hit_for_cacheable_get() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/assets/app.js?v=1")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    let context = match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await
    {
        CacheLookup::Miss(context) => *context,
        CacheLookup::Hit(_) => panic!("empty cache should miss"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    };

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .header(CONTENT_TYPE, "text/javascript")
        .body(full_body("cached body"))
        .expect("response should build");
    let stored = manager.store_response(context, response).await;
    assert_eq!(stored.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            assert_eq!(response.headers().get(CACHE_STATUS_HEADER).unwrap(), "HIT");
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"cached body");
        }
        CacheLookup::Miss(_) => panic!("stored response should hit"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }
}

#[tokio::test]
async fn cache_manager_does_not_store_entries_over_max_entry_size() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 4);
    let policy = test_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/large")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let context = match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await
    {
        CacheLookup::Miss(context) => *context,
        CacheLookup::Hit(_) => panic!("empty cache should miss"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    };

    let response = Response::builder()
        .status(StatusCode::OK)
        .body(full_body("too large"))
        .expect("response should build");
    let stored = manager.store_response(context, response).await;
    assert_eq!(stored.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(_) => {}
        CacheLookup::Hit(_) => panic!("oversized response should not be cached"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }
}

#[tokio::test]
async fn cache_manager_skips_unknown_size_response_without_consuming_body() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/streamed")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let context = match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await
    {
        CacheLookup::Miss(context) => *context,
        CacheLookup::Hit(_) => panic!("empty cache should miss"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    };

    let stream = stream::iter([Ok::<Frame<Bytes>, BoxError>(Frame::data(Bytes::from_static(
        b"streamed body",
    )))]);
    let response = Response::builder()
        .status(StatusCode::OK)
        .body(boxed_body(StreamBody::new(stream)))
        .expect("response should build");
    let stored = manager.store_response(context, response).await;
    let body = stored.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"streamed body");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(_) => {}
        CacheLookup::Hit(_) => panic!("unknown-size response should not be cached"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }
}

#[tokio::test]
async fn refresh_not_modified_response_serves_body_and_evicts_uncacheable_entry() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path().to_path_buf(), 1024);
    let key = "https:example.com:/refresh";
    let hash = cache_key_hash(key);
    let now = unix_time_ms(SystemTime::now());
    let cached_metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        &http::HeaderMap::new(),
        test_metadata_input(now.saturating_sub(2_000), now.saturating_sub(1_000), 6),
    );
    let paths = cache_paths(temp.path(), &hash);
    write_cache_entry(&paths, &cached_metadata, b"cached").await.expect("entry should be written");
    let cached_entry =
        test_index_entry(hash.clone(), 6, now.saturating_sub(1_000), now.saturating_sub(2_000));
    {
        let mut index = lock_index(&zone.index);
        index.entries.insert(key.to_string(), cached_entry.clone());
        index.current_size_bytes = 6;
    }

    let mut context = test_store_context(zone.clone(), key);
    context.cached_entry = Some(cached_entry);
    context.cached_metadata = Some(cached_metadata);
    context.cache_status = CacheStatus::Expired;

    let response = Response::builder()
        .status(StatusCode::NOT_MODIFIED)
        .header(CACHE_CONTROL, "no-store")
        .body(full_body(Bytes::new()))
        .expect("304 response should build");
    let refreshed = refresh_not_modified_response(context, response)
        .await
        .expect("revalidation should reuse cached body");

    assert_eq!(refreshed.headers().get(CACHE_STATUS_HEADER).unwrap(), "REVALIDATED");
    assert_eq!(refreshed.headers().get(CACHE_CONTROL).unwrap(), "no-store");
    let body = refreshed.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"cached");
    assert!(!lock_index(&zone.index).entries.contains_key(key));
    assert!(!paths.metadata.exists());
    assert!(!paths.body.exists());
    assert_eq!(zone.stats.revalidated_total.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(zone.stats.write_success_total.load(std::sync::atomic::Ordering::Relaxed), 0);
}

#[test]
fn load_index_from_disk_keeps_legacy_expired_entries_without_stale_windows() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path().to_path_buf(), 1024);
    let key = "https:example.com:/legacy";
    let hash = cache_key_hash(key);
    let paths = cache_paths(temp.path(), &hash);
    let now = unix_time_ms(SystemTime::now());
    std::fs::create_dir_all(&paths.dir).expect("cache dir should be created");
    std::fs::write(
        &paths.metadata,
        serde_json::to_vec(&serde_json::json!({
            "key": key,
            "stored_at_unix_ms": now.saturating_sub(2_000),
            "expires_at_unix_ms": now.saturating_sub(1_000),
            "must_revalidate": false,
            "body_size_bytes": 6
        }))
        .expect("legacy metadata should serialize"),
    )
    .expect("legacy metadata should be written");
    std::fs::write(&paths.body, b"cached").expect("legacy body should be written");

    let index = load_index_from_disk(zone.config.as_ref()).expect("index should load");
    let entry = index.entries.get(key).expect("legacy entry should be retained");

    assert_eq!(entry.hash, hash);
    assert_eq!(entry.body_size_bytes, 6);
    assert_eq!(index.current_size_bytes, 6);
    assert!(paths.metadata.exists());
    assert!(paths.body.exists());
}
