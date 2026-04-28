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
        min_uses: 1,
        ignore_headers: Vec::new(),
        range_requests: rginx_core::CacheRangeRequestPolicy::Bypass,
        slice_size_bytes: None,
        convert_head: true,
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
        min_uses: 1,
        ignore_headers: Vec::new(),
        range_requests: rginx_core::CacheRangeRequestPolicy::Bypass,
        slice_size_bytes: None,
        convert_head: true,
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
        min_uses: 1,
        ignore_headers: Vec::new(),
        range_requests: rginx_core::CacheRangeRequestPolicy::Bypass,
        slice_size_bytes: None,
        convert_head: true,
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
        min_uses: 1,
        ignore_headers: Vec::new(),
        range_requests: rginx_core::CacheRangeRequestPolicy::Bypass,
        slice_size_bytes: None,
        convert_head: true,
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
        min_uses: 1,
        ignore_headers: Vec::new(),
        range_requests: rginx_core::CacheRangeRequestPolicy::Bypass,
        slice_size_bytes: None,
        convert_head: true,
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

#[test]
fn range_request_bypasses_cache_by_default() {
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
        min_uses: 1,
        ignore_headers: Vec::new(),
        range_requests: rginx_core::CacheRangeRequestPolicy::Bypass,
        slice_size_bytes: None,
        convert_head: true,
    };
    let request = Request::builder()
        .method(Method::GET)
        .uri("/video.mp4")
        .header(http::header::RANGE, "bytes=0-99")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let request = CacheRequest::from_request(&request);

    assert!(cache_request_bypass(&request, &policy));
}

#[test]
fn cache_key_includes_range_when_enabled() {
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
        min_uses: 1,
        ignore_headers: Vec::new(),
        range_requests: rginx_core::CacheRangeRequestPolicy::Cache,
        slice_size_bytes: None,
        convert_head: true,
    };
    let request = Request::builder()
        .method(Method::GET)
        .uri("/video.mp4")
        .header("host", "example.com")
        .header(http::header::RANGE, "bytes=0-99")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    assert_eq!(
        render_cache_key(request.method(), request.uri(), request.headers(), "https", &policy),
        "https:example.com:/video.mp4|range:0-99"
    );
}

#[test]
fn multiple_range_headers_bypass_cache_when_range_caching_is_enabled() {
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
        min_uses: 1,
        ignore_headers: Vec::new(),
        range_requests: rginx_core::CacheRangeRequestPolicy::Cache,
        slice_size_bytes: None,
        convert_head: true,
    };
    let request = Request::builder()
        .method(Method::GET)
        .uri("/video.mp4")
        .header(http::header::RANGE, "bytes=0-99")
        .header(http::header::RANGE, "bytes=100-199")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let request = CacheRequest::from_request(&request);

    assert!(cache_request_bypass(&request, &policy));
}
