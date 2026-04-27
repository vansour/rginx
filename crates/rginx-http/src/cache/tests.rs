use super::*;
use http::header::{CACHE_CONTROL, CONTENT_TYPE};

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
    })
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
    fs::create_dir_all(&paths.dir).await.expect("cache dir should be created");
    fs::write(&paths.metadata, b"{not-json").await.expect("metadata should be written");
    fs::write(&paths.body, b"cached").await.expect("body should be written");

    let zone = manager.zones.get("default").expect("zone should exist");
    {
        let mut index = lock_index(&zone.index);
        index.entries.insert(
            key.to_string(),
            CacheIndexEntry {
                hash,
                body_size_bytes: 6,
                expires_at_unix_ms: unix_time_ms(SystemTime::now()) + 60_000,
                last_access_unix_ms: unix_time_ms(SystemTime::now()),
            },
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
    assert!(lock_index(&zone.index).entries.get(key).is_none());
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

#[test]
fn response_ttl_respects_explicit_zero_or_expired_freshness() {
    let mut headers = HeaderMap::new();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("max-age=0"));
    assert_eq!(response_ttl(&headers, Duration::from_secs(60)), Duration::ZERO);

    let mut headers = HeaderMap::new();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("max-age=60, s-maxage=10"));
    assert_eq!(response_ttl(&headers, Duration::from_secs(1)), Duration::from_secs(10));

    let mut headers = HeaderMap::new();
    headers.insert(EXPIRES, HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT"));
    assert_eq!(response_ttl(&headers, Duration::from_secs(60)), Duration::ZERO);
}

#[test]
fn no_store_response_is_not_storable() {
    let zone = Arc::new(CacheZoneRuntime {
        config: Arc::new(CacheZone {
            name: "default".to_string(),
            path: PathBuf::from("/tmp/rginx-cache-test"),
            max_size_bytes: None,
            inactive: Duration::from_secs(60),
            default_ttl: Duration::from_secs(60),
            max_entry_bytes: 1024,
        }),
        index: Mutex::new(CacheIndex::default()),
    });
    let context = CacheStoreContext {
        zone,
        policy: RouteCachePolicy {
            zone: "default".to_string(),
            methods: vec![Method::GET],
            statuses: vec![StatusCode::OK],
            key: rginx_core::CacheKeyTemplate::parse("{uri}").expect("key should parse"),
            stale_if_error: None,
        },
        key: "/".to_string(),
        cache_status: CacheStatus::Miss,
        store_response: true,
    };
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "no-store")
        .header(CONTENT_TYPE, "text/plain")
        .body(full_body("hello"))
        .expect("response should build");

    assert!(!response_is_storable(&context, &response));
}
