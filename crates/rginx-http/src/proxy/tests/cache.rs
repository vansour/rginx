use super::*;

fn cache_policy() -> rginx_core::RouteCachePolicy {
    rginx_core::RouteCachePolicy {
        zone: "default".to_string(),
        methods: vec![Method::GET, Method::HEAD],
        statuses: vec![StatusCode::OK],
        key: rginx_core::CacheKeyTemplate::parse("{scheme}:{host}:{uri}")
            .expect("cache key should parse"),
        stale_if_error: None,
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
async fn forward_request_uses_route_cache_for_miss_then_hit() {
    let statuses =
        Arc::new(Mutex::new(VecDeque::from([StatusCode::OK, StatusCode::INTERNAL_SERVER_ERROR])));
    let _server = spawn_status_server(statuses.clone()).await;
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
            default_ttl: Duration::from_secs(60),
            max_entry_bytes: 1024,
        }),
    );
    let state = crate::state::SharedState::from_config(snapshot).expect("state should build");
    let active = state.snapshot().await;
    let target = proxy_target(upstream);

    let first = crate::proxy::forward_request(
        state.clone(),
        active.clone(),
        get_request("/cached"),
        "default",
        &target,
        client_address(),
        downstream_context("cache-miss", Some(cache_policy())),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.headers().get("x-cache").unwrap(), "MISS");
    let first_body = first.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(first_body.as_ref(), b"ok");

    let second = crate::proxy::forward_request(
        state,
        active,
        get_request("/cached"),
        "default",
        &target,
        client_address(),
        downstream_context("cache-hit", Some(cache_policy())),
    )
    .await;
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(second.headers().get("x-cache").unwrap(), "HIT");
    let second_body = second.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(second_body.as_ref(), b"ok");
}

#[tokio::test]
async fn forward_request_marks_authorization_request_as_cache_bypass() {
    let statuses = Arc::new(Mutex::new(VecDeque::from([StatusCode::OK])));
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
            default_ttl: Duration::from_secs(60),
            max_entry_bytes: 1024,
        }),
    );
    let state = crate::state::SharedState::from_config(snapshot).expect("state should build");
    let active = state.snapshot().await;
    let target = proxy_target(upstream);
    let request = http::Request::builder()
        .method(Method::GET)
        .uri("/private")
        .header(HOST, HeaderValue::from_static("example.com"))
        .header(http::header::AUTHORIZATION, HeaderValue::from_static("Bearer token"))
        .body(crate::handler::full_body(Bytes::new()))
        .expect("request should build");

    let response = crate::proxy::forward_request(
        state,
        active,
        request,
        "default",
        &target,
        client_address(),
        downstream_context("cache-bypass", Some(cache_policy())),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("x-cache").unwrap(), "BYPASS");
}
