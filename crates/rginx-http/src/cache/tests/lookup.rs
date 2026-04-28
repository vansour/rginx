use std::time::{Duration, SystemTime};

use bytes::Bytes;
use http::header::{ACCEPT_ENCODING, AUTHORIZATION, CACHE_CONTROL};
use http::{Method, Request, StatusCode};
use tokio::sync::Notify;

use crate::handler::full_body;

use super::*;

#[test]
fn cache_key_template_renders_request_parts() {
    let template =
        rginx_core::CacheKeyTemplate::parse("{scheme}:{host}:{uri}").expect("key should parse");
    let policy = RouteCachePolicy {
        zone: "default".to_string(),
        methods: vec![Method::GET],
        statuses: vec![StatusCode::OK],
        ttl_by_status: Vec::new(),
        key: template,
        cache_bypass: None,
        no_cache: None,
        stale_if_error: None,
        use_stale: Vec::new(),
        background_update: false,
        lock_timeout: Duration::from_secs(5),
        lock_age: Duration::from_secs(5),
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
fn cache_key_includes_all_accept_encoding_header_values() {
    let policy = RouteCachePolicy {
        zone: "default".to_string(),
        methods: vec![Method::GET],
        statuses: vec![StatusCode::OK],
        ttl_by_status: Vec::new(),
        key: rginx_core::CacheKeyTemplate::parse("{scheme}:{host}:{uri}")
            .expect("key should parse"),
        cache_bypass: None,
        no_cache: None,
        stale_if_error: None,
        use_stale: Vec::new(),
        background_update: false,
        lock_timeout: Duration::from_secs(5),
        lock_age: Duration::from_secs(5),
    };
    let request = Request::builder()
        .method(Method::GET)
        .uri("/assets/app.js?v=1")
        .header("host", "example.com")
        .header(ACCEPT_ENCODING, "gzip")
        .header(ACCEPT_ENCODING, "br;q=1")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    assert_eq!(
        render_cache_key(request.method(), request.uri(), request.headers(), "https", &policy),
        "https:example.com:/assets/app.js?v=1|ae:gzip,br;q=1"
    );
}

#[test]
fn cache_key_template_renders_header_query_and_cookie_variables() {
    let policy = RouteCachePolicy {
        zone: "default".to_string(),
        methods: vec![Method::GET],
        statuses: vec![StatusCode::OK],
        ttl_by_status: Vec::new(),
        key: rginx_core::CacheKeyTemplate::parse(
            "{header:accept-language}:{query:lang}:{cookie:session}",
        )
        .expect("key should parse"),
        cache_bypass: None,
        no_cache: None,
        stale_if_error: None,
        use_stale: Vec::new(),
        background_update: false,
        lock_timeout: Duration::from_secs(5),
        lock_age: Duration::from_secs(5),
    };
    let request = Request::builder()
        .method(Method::GET)
        .uri("/assets/app.js?lang=zh-CN")
        .header("accept-language", "zh-CN")
        .header("cookie", "session=abc123; theme=light")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    assert_eq!(
        render_cache_key(request.method(), request.uri(), request.headers(), "https", &policy),
        "zh-CN:zh-CN:abc123"
    );
}

#[test]
fn authorization_request_bypasses_cache() {
    let policy = RouteCachePolicy {
        zone: "default".to_string(),
        methods: vec![Method::GET],
        statuses: vec![StatusCode::OK],
        ttl_by_status: Vec::new(),
        key: rginx_core::CacheKeyTemplate::parse("{uri}").expect("key should parse"),
        cache_bypass: None,
        no_cache: None,
        stale_if_error: None,
        use_stale: Vec::new(),
        background_update: false,
        lock_timeout: Duration::from_secs(5),
        lock_age: Duration::from_secs(5),
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

#[test]
fn configured_header_bypasses_cache() {
    let policy = RouteCachePolicy {
        zone: "default".to_string(),
        methods: vec![Method::GET],
        statuses: vec![StatusCode::OK],
        ttl_by_status: Vec::new(),
        key: rginx_core::CacheKeyTemplate::parse("{uri}").expect("key should parse"),
        cache_bypass: Some(rginx_core::CachePredicate::HeaderExists(
            http::header::HeaderName::from_static("x-cache-bypass"),
        )),
        no_cache: None,
        stale_if_error: None,
        use_stale: Vec::new(),
        background_update: false,
        lock_timeout: Duration::from_secs(5),
        lock_age: Duration::from_secs(5),
    };
    let request = Request::builder()
        .method(Method::GET)
        .uri("/")
        .header("x-cache-bypass", "1")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let request = CacheRequest::from_request(&request);

    assert!(cache_request_bypass(&request, &policy));
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
        CacheLookup::Updating(_, _) => {
            panic!("corrupt metadata must not trigger background update")
        }
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }

    assert!(!lock_index(&zone.index).entries.contains_key(key));
}

#[tokio::test]
async fn cache_manager_treats_metadata_key_mismatch_as_miss() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let key = "https:example.com:/mismatch";
    let hash = cache_key_hash(key);
    let paths = cache_paths(temp.path(), &hash);
    let now = unix_time_ms(SystemTime::now());
    let metadata = cache_metadata(
        "https:example.com:/other".to_string(),
        StatusCode::OK,
        &http::HeaderMap::new(),
        test_metadata_input(now.saturating_sub(2_000), now.saturating_add(60_000), 6),
    );
    write_cache_entry(&paths, &metadata, b"cached").await.expect("entry should be written");

    let zone = manager.zones.get("default").expect("zone should exist");
    {
        let mut index = lock_index(&zone.index);
        index.entries.insert(
            key.to_string(),
            test_index_entry(hash, 6, now.saturating_add(60_000), now.saturating_sub(1_000)),
        );
        index.current_size_bytes = 6;
    }

    let request = Request::builder()
        .method(Method::GET)
        .uri("/mismatch")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(_) => {}
        CacheLookup::Hit(_) => panic!("mismatched metadata key must not be served"),
        CacheLookup::Updating(_, _) => {
            panic!("mismatched metadata key must not trigger background update")
        }
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }

    assert!(!lock_index(&zone.index).entries.contains_key(key));
}

#[tokio::test]
async fn cache_manager_retains_expired_entries_for_revalidation_on_lookup() {
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
        test_metadata_input(now.saturating_sub(2_000), now.saturating_sub(1_000), 7),
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
        CacheLookup::Updating(_, _) => panic!("expired response should not update without policy"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }

    let index = lock_index(&zone.index);
    assert!(index.entries.contains_key(key));
    assert_eq!(index.current_size_bytes, 7);
    assert!(paths.metadata.exists());
    assert!(paths.body.exists());
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
        CacheLookup::Miss(context) => *context,
        CacheLookup::Hit(_) => panic!("empty cache should miss"),
        CacheLookup::Updating(_, _) => panic!("empty cache should not update"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    };
    let response = http::Response::builder()
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
            let body =
                http_body_util::BodyExt::collect(response.into_body()).await.unwrap().to_bytes();
            assert!(body.is_empty());
        }
        CacheLookup::Miss(_) => panic!("HEAD should not need the cached body file"),
        CacheLookup::Updating(_, _) => panic!("HEAD hit should not trigger background update"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }
}

#[test]
fn should_refresh_from_not_modified_requires_cached_entry() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path().to_path_buf(), 1024);
    let mut context = test_store_context(zone, "/revalidate");

    assert!(!context.should_refresh_from_not_modified(StatusCode::NOT_MODIFIED));

    context.cached_entry = Some(test_index_entry("hash".to_string(), 6, 0, 0));
    assert!(context.should_refresh_from_not_modified(StatusCode::NOT_MODIFIED));
    assert!(!context.should_refresh_from_not_modified(StatusCode::OK));
}

#[test]
fn no_store_headers_remain_non_storable_during_revalidation() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path().to_path_buf(), 1024);
    let context = test_store_context(zone, "/");
    let response = http::Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "no-store")
        .body(full_body("hello"))
        .expect("response should build");

    assert!(!response_is_storable(&context, &response));
}

#[tokio::test]
async fn cache_manager_returns_updating_for_background_refresh() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.background_update = true;
    policy.use_stale = vec![rginx_core::CacheUseStaleCondition::Updating];

    let key = "https:example.com:/background";
    let hash = cache_key_hash(key);
    let paths = cache_paths(temp.path(), &hash);
    let now = unix_time_ms(SystemTime::now());
    let metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        &http::HeaderMap::new(),
        test_metadata_input(now.saturating_sub(2_000), now.saturating_sub(1_000), 7),
    );
    write_cache_entry(&paths, &metadata, b"cached!").await.expect("entry should be written");
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
        .uri("/background")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Updating(response, context) => {
            assert_eq!(response.headers().get(CACHE_STATUS_HEADER).unwrap(), "UPDATING");
            assert_eq!(context.cache_status(), CacheStatus::Updating);
        }
        _ => panic!("expected background update"),
    }
}

#[tokio::test]
async fn cache_manager_lock_timeout_falls_back_to_bypass() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.lock_timeout = Duration::from_millis(1);
    let key = "https:example.com:/locked";
    let zone = manager.zones.get("default").expect("zone should exist");
    zone.fill_locks.lock().unwrap().insert(
        key.to_string(),
        CacheFillLockState {
            notify: Arc::new(Notify::new()),
            acquired_at_unix_ms: unix_time_ms(SystemTime::now()),
            generation: 1,
        },
    );

    let request = Request::builder()
        .method(Method::GET)
        .uri("/locked")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(context) => {
            assert_eq!(context.cache_status(), CacheStatus::Bypass);
        }
        _ => panic!("expected timeout bypass miss"),
    }
}

#[tokio::test]
async fn cache_manager_lock_age_allows_second_fill() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.lock_age = Duration::from_millis(1);
    let key = "https:example.com:/aged";
    let zone = manager.zones.get("default").expect("zone should exist");
    zone.fill_locks.lock().unwrap().insert(
        key.to_string(),
        CacheFillLockState {
            notify: Arc::new(Notify::new()),
            acquired_at_unix_ms: unix_time_ms(SystemTime::now()).saturating_sub(5_000),
            generation: 1,
        },
    );

    let request = Request::builder()
        .method(Method::GET)
        .uri("/aged")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(context) => assert_eq!(context.cache_status(), CacheStatus::Miss),
        _ => panic!("expected fresh fill acquisition"),
    }
}
