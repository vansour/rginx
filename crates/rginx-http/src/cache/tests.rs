use super::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use futures_util::stream;
use http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, EXPIRES};
use http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, StreamBody};
use hyper::body::Frame;
use rginx_core::{CacheZone, RouteCachePolicy};

use crate::handler::{BoxError, boxed_body, full_body};

fn test_zone(path: PathBuf, max_entry_bytes: usize) -> Arc<CacheZoneRuntime> {
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
        change_notifier: None,
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

#[test]
fn cache_key_template_renders_request_parts() {
    let template =
        rginx_core::CacheKeyTemplate::parse("{scheme}:{host}:{uri}").expect("key should parse");
    let policy = RouteCachePolicy {
        zone: "default".to_string(),
        methods: vec![Method::GET],
        statuses: vec![StatusCode::OK],
        key: template,
        stale_if_error: None,
    };
    let request = Request::builder()
        .method(Method::GET)
        .uri("/assets/app.js?v=1")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    assert_eq!(
        render_cache_key(request.method(), request.uri(), request.headers(), "https", &policy),
        "https:example.com:/assets/app.js?v=1"
    );
}

#[test]
fn authorization_request_bypasses_cache() {
    let policy = RouteCachePolicy {
        zone: "default".to_string(),
        methods: vec![Method::GET],
        statuses: vec![StatusCode::OK],
        key: rginx_core::CacheKeyTemplate::parse("{uri}").expect("key should parse"),
        stale_if_error: None,
    };
    let request = Request::builder()
        .method(Method::GET)
        .uri("/")
        .header(AUTHORIZATION, "Bearer token")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let request = CacheRequest::from_request(&request);

    assert!(cache_request_bypass(&request, &policy));
}

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
        CacheLookup::Miss(context) => context,
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
async fn cache_manager_treats_corrupt_metadata_as_miss() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let key = "https:example.com:/broken";
    let hash = cache_key_hash(key);
    let paths = cache_paths(temp.path(), &hash);
    tokio::fs::create_dir_all(&paths.dir).await.expect("cache dir should be created");
    tokio::fs::write(&paths.metadata, b"{not-json").await.expect("metadata should be written");
    tokio::fs::write(&paths.body, b"cached").await.expect("body should be written");

    let zone = manager.zones.get("default").expect("zone should exist");
    {
        let mut index = lock_index(&zone.index);
        index.entries.insert(
            key.to_string(),
            test_index_entry(
                hash,
                6,
                unix_time_ms(SystemTime::now()) + 60_000,
                unix_time_ms(SystemTime::now()),
            ),
        );
        index.current_size_bytes = 6;
    }

    let request = Request::builder()
        .method(Method::GET)
        .uri("/broken")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(_) => {}
        CacheLookup::Hit(_) => panic!("corrupt metadata must not be served"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }

    let zone = manager.zones.get("default").expect("zone should exist");
    assert!(!lock_index(&zone.index).entries.contains_key(key));
}

#[tokio::test]
async fn cache_manager_removes_expired_entries_on_lookup() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let key = "https:example.com:/expired";
    let hash = cache_key_hash(key);
    let paths = cache_paths(temp.path(), &hash);
    let now = unix_time_ms(SystemTime::now());
    let metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        &http::HeaderMap::new(),
        now.saturating_sub(2_000),
        now.saturating_sub(1_000),
        None,
        None,
        false,
        7,
    );
    write_cache_entry(&paths, &metadata, b"expired").await.expect("entry should be written");
    let zone = manager.zones.get("default").expect("zone should exist");
    {
        let mut index = lock_index(&zone.index);
        index.entries.insert(
            key.to_string(),
            test_index_entry(hash.clone(), 7, now.saturating_sub(1_000), now.saturating_sub(2_000)),
        );
        index.current_size_bytes = 7;
    }
    let request = Request::builder()
        .method(Method::GET)
        .uri("/expired")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(context) => assert_eq!(context.cache_status(), CacheStatus::Expired),
        CacheLookup::Hit(_) => panic!("expired response must not be served"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }

    let zone = manager.zones.get("default").expect("zone should exist");
    let index = lock_index(&zone.index);
    assert!(index.entries.contains_key(key));
    assert_eq!(index.current_size_bytes, 7);
    assert!(paths.metadata.exists());
    assert!(paths.body.exists());
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
        CacheLookup::Miss(context) => context,
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
        CacheLookup::Miss(context) => context,
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
async fn cache_manager_serves_head_hit_without_reading_body_file() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let get_request = Request::builder()
        .method(Method::GET)
        .uri("/head")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let context = match manager
        .lookup(CacheRequest::from_request(&get_request), "https", &policy)
        .await
    {
        CacheLookup::Miss(context) => context,
        CacheLookup::Hit(_) => panic!("empty cache should miss"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    };
    let response = Response::builder()
        .status(StatusCode::OK)
        .body(full_body("cached body"))
        .expect("response should build");
    let _ = manager.store_response(context, response).await;

    let hash = cache_key_hash("https:example.com:/head");
    let paths = cache_paths(temp.path(), &hash);
    tokio::fs::remove_file(paths.body).await.expect("body file should be removable");

    let head_request = Request::builder()
        .method(Method::HEAD)
        .uri("/head")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    match manager.lookup(CacheRequest::from_request(&head_request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            assert_eq!(response.headers().get(CACHE_STATUS_HEADER).unwrap(), "HIT");
            assert_eq!(response.headers().get(http::header::CONTENT_LENGTH).unwrap(), "11");
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert!(body.is_empty());
        }
        CacheLookup::Miss(_) => panic!("HEAD should not need the cached body file"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }
}

#[test]
fn response_ttl_respects_explicit_zero_or_expired_freshness() {
    let mut headers = HeaderMap::new();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("max-age=0"));
    assert_eq!(response_ttl(&headers, Duration::from_secs(60)), Duration::ZERO);

    let mut headers = HeaderMap::new();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("max-age=60, s-maxage=10"));
    assert_eq!(response_ttl(&headers, Duration::from_secs(1)), Duration::from_secs(10));

    let mut headers = HeaderMap::new();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("max-age=invalid, s-maxage=120"));
    assert_eq!(response_ttl(&headers, Duration::from_secs(1)), Duration::from_secs(120));

    let mut headers = HeaderMap::new();
    headers.insert(EXPIRES, HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT"));
    assert_eq!(response_ttl(&headers, Duration::from_secs(60)), Duration::ZERO);
}

#[test]
fn no_store_response_is_not_storable() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = Arc::new(CacheZoneRuntime {
        config: Arc::new(CacheZone {
            name: "default".to_string(),
            path: temp.path().to_path_buf(),
            max_size_bytes: None,
            inactive: Duration::from_secs(60),
            default_ttl: Duration::from_secs(60),
            max_entry_bytes: 1024,
        }),
        index: Mutex::new(CacheIndex::default()),
        io_lock: AsyncMutex::new(()),
        fill_locks: Arc::new(Mutex::new(HashMap::new())),
        stats: CacheZoneStats::default(),
        change_notifier: None,
    });
    let context = test_store_context(zone, "/");
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "no-store")
        .header(CONTENT_TYPE, "text/plain")
        .body(full_body("hello"))
        .expect("response should build");

    assert!(!response_is_storable(&context, &response));
}
