use super::*;

fn cache_policy() -> rginx_core::RouteCachePolicy {
    rginx_core::RouteCachePolicy {
        zone: "default".to_string(),
        methods: vec![Method::GET, Method::HEAD],
        statuses: vec![StatusCode::OK],
        ttl_by_status: Vec::new(),
        key: rginx_core::CacheKeyTemplate::parse("{scheme}:{host}:{uri}")
            .expect("cache key should parse"),
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
    }
}

fn client_address() -> ClientAddress {
    ClientAddress {
        peer_addr: "198.51.100.10:49152".parse().unwrap(),
        client_ip: "198.51.100.10".parse().unwrap(),
        forwarded_for: "198.51.100.10".to_string(),
        source: ClientIpSource::SocketPeer,
    }
}

fn downstream_context<'a>(
    request_id: &'a str,
    cache: Option<rginx_core::RouteCachePolicy>,
) -> crate::proxy::DownstreamRequestContext<'a> {
    crate::proxy::DownstreamRequestContext {
        listener_id: "default",
        downstream_proto: "http",
        request_id,
        options: crate::proxy::DownstreamRequestOptions {
            request_body_read_timeout: None,
            max_request_body_bytes: None,
            request_buffering: rginx_core::RouteBufferingPolicy::Auto,
            response_buffering: rginx_core::RouteBufferingPolicy::Auto,
            streaming_response_idle_timeout: None,
            cache,
        },
    }
}

fn proxy_target(upstream: Arc<Upstream>) -> rginx_core::ProxyTarget {
    rginx_core::ProxyTarget {
        upstream_name: "backend".to_string(),
        upstream,
        preserve_host: false,
        strip_prefix: None,
        proxy_set_headers: Vec::new(),
    }
}

fn get_request(path: &str) -> http::Request<crate::handler::HttpBody> {
    http::Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(HOST, HeaderValue::from_static("example.com"))
        .body(crate::handler::full_body(Bytes::new()))
        .expect("request should build")
}

#[tokio::test]
async fn forward_request_serves_stale_for_configured_429() {
    let statuses =
        Arc::new(Mutex::new(VecDeque::from([StatusCode::OK, StatusCode::TOO_MANY_REQUESTS])));
    let _server = spawn_status_server(statuses).await;
    let upstream = Arc::new(Upstream::new(
        "backend".to_string(),
        vec![peer(&format!("http://{}", _server.listen_addr))],
        rginx_core::UpstreamTls::NativeRoots,
        upstream_settings(UpstreamProtocol::Auto),
    ));
    let mut snapshot = snapshot_with_upstream("backend", upstream.clone());
    let temp = TempDir::new().expect("cache temp dir should exist");
    snapshot.cache_zones.insert(
        "default".to_string(),
        Arc::new(rginx_core::CacheZone {
            name: "default".to_string(),
            path: temp.path().to_path_buf(),
            max_size_bytes: Some(1024 * 1024),
            inactive: Duration::from_secs(60),
            default_ttl: Duration::from_millis(1),
            max_entry_bytes: 1024,
            path_levels: vec![2],
            loader_batch_entries: 100,
            loader_sleep: Duration::ZERO,
            manager_batch_entries: 100,
            manager_sleep: Duration::ZERO,
            inactive_cleanup_interval: Duration::from_secs(60),
            shared_index: true,
        }),
    );
    let state = crate::state::SharedState::from_config(snapshot).expect("state should build");
    let active = state.snapshot().await;
    let target = proxy_target(upstream);
    let mut policy = cache_policy();
    policy.use_stale = vec![rginx_core::CacheUseStaleCondition::Http429];

    let first = crate::proxy::forward_request(
        state.clone(),
        active.clone(),
        get_request("/stale-429"),
        "default",
        &target,
        client_address(),
        downstream_context("cache-429-fill", Some(policy.clone())),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.headers().get("x-cache").unwrap(), "MISS");
    let first_body = first.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(first_body.as_ref(), b"ok");

    let second = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let response = crate::proxy::forward_request(
                state.clone(),
                active.clone(),
                get_request("/stale-429"),
                "default",
                &target,
                client_address(),
                downstream_context("cache-429-stale", Some(policy.clone())),
            )
            .await;
            if response.headers().get("x-cache").is_some_and(|value| value == "STALE") {
                break response;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cache entry should eventually expire and serve stale on 429");
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(second.headers().get("x-cache").unwrap(), "STALE");
    let body = second.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"ok");
}
