use super::*;

#[test]
fn compile_supports_nezha_dashboard_native_vhost_shape() {
    let base_dir = temp_base_dir("rginx-vhost-nezha-");
    fs::write(base_dir.path().join("dashboard.crt"), b"placeholder")
        .expect("cert should be written");
    fs::write(base_dir.path().join("dashboard.key"), b"placeholder")
        .expect("key should be written");

    let mut server = server_defaults(None);
    server.trusted_proxies = vec!["203.0.113.0/24".to_string(), "2001:db8:1234::/48".to_string()];
    server.client_ip_header = Some("CF-Connecting-IP".to_string());

    let mut dashboard_grpc = upstream("dashboard_grpc", "http://127.0.0.1:8008");
    dashboard_grpc.protocol = UpstreamProtocolConfig::H2c;
    dashboard_grpc.pool_max_idle_per_host = Some(512);
    dashboard_grpc.read_timeout_secs = Some(600);
    dashboard_grpc.write_timeout_secs = Some(600);
    dashboard_grpc.http2_keep_alive_interval_secs = Some(30);

    let mut dashboard_http = upstream("dashboard_http", "http://127.0.0.1:8008");
    dashboard_http.pool_max_idle_per_host = Some(512);
    dashboard_http.read_timeout_secs = Some(3600);
    dashboard_http.write_timeout_secs = Some(3600);

    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server,
        upstreams: Vec::new(),
        locations: Vec::new(),
        servers: vec![VirtualHostConfig {
            listen: vec![
                "127.0.0.1:8443 ssl http2".to_string(),
                "[::1]:8443 ssl http2".to_string(),
            ],
            server_names: vec!["dashboard.example.com".to_string()],
            upstreams: vec![dashboard_http, dashboard_grpc],
            locations: vec![
                test_location(
                    MatcherConfig::Prefix("/proto.NezhaService/".to_string()),
                    HandlerConfig::Proxy {
                        upstream: "dashboard_grpc".to_string(),
                        preserve_host: Some(true),
                        strip_prefix: None,
                        proxy_set_headers: std::collections::HashMap::from([(
                            "nz-realip".to_string(),
                            ProxyHeaderValueConfig::Dynamic(
                                ProxyHeaderDynamicValueConfig::ClientIp,
                            ),
                        )]),
                    },
                ),
                LocationConfig {
                    matcher: MatcherConfig::Regex {
                        pattern: "^/api/v1/ws/(server|terminal|file)(/.*)?$".to_string(),
                        case_insensitive: true,
                    },
                    handler: HandlerConfig::Proxy {
                        upstream: "dashboard_http".to_string(),
                        preserve_host: Some(true),
                        strip_prefix: None,
                        proxy_set_headers: std::collections::HashMap::from([
                            (
                                "nz-realip".to_string(),
                                ProxyHeaderValueConfig::Dynamic(
                                    ProxyHeaderDynamicValueConfig::ClientIp,
                                ),
                            ),
                            (
                                "origin".to_string(),
                                ProxyHeaderValueConfig::Dynamic(
                                    ProxyHeaderDynamicValueConfig::Template(
                                        "https://{host}".to_string(),
                                    ),
                                ),
                            ),
                        ]),
                    },
                    grpc_service: None,
                    grpc_method: None,
                    allow_cidrs: Vec::new(),
                    deny_cidrs: Vec::new(),
                    requests_per_sec: None,
                    burst: None,
                    allow_early_data: None,
                    request_buffering: None,
                    response_buffering: Some(RouteBufferingPolicyConfig::Off),
                    compression: Some(RouteCompressionPolicyConfig::Off),
                    compression_min_bytes: None,
                    compression_content_types: None,
                    streaming_response_idle_timeout_secs: None,
                },
                test_location(
                    MatcherConfig::Prefix("/".to_string()),
                    HandlerConfig::Proxy {
                        upstream: "dashboard_http".to_string(),
                        preserve_host: Some(true),
                        strip_prefix: None,
                        proxy_set_headers: std::collections::HashMap::from([(
                            "nz-realip".to_string(),
                            ProxyHeaderValueConfig::Dynamic(
                                ProxyHeaderDynamicValueConfig::ClientIp,
                            ),
                        )]),
                    },
                ),
            ],
            tls: Some(crate::model::VirtualHostTlsConfig {
                cert_path: "dashboard.crt".to_string(),
                key_path: "dashboard.key".to_string(),
                additional_certificates: None,
                ocsp_staple_path: None,
                ocsp: None,
            }),
            http3: None,
        }],
    };

    crate::validate::validate(&config)
        .expect("Nezha dashboard native vhost config should validate");

    let snapshot = compile_with_base(config, base_dir.path())
        .expect("Nezha dashboard native vhost config should compile");

    assert_eq!(snapshot.listeners.len(), 2);
    assert!(snapshot.listeners.iter().all(rginx_core::Listener::tls_enabled));
    assert!(snapshot.listeners.iter().all(|listener| {
        listener.server.client_ip_header.as_ref().map(|name| name.as_str())
            == Some("cf-connecting-ip")
    }));

    let vhost = &snapshot.vhosts[0];
    assert_eq!(vhost.server_names, vec!["dashboard.example.com"]);
    assert_eq!(vhost.routes.len(), 3);

    let grpc_route = vhost
        .routes
        .iter()
        .find(|route| matches!(route.matcher, rginx_core::RouteMatcher::Prefix(ref path) if path == "/proto.NezhaService/"))
        .expect("gRPC prefix route should compile");
    let rginx_core::RouteAction::Proxy(grpc_proxy) = &grpc_route.action else {
        panic!("gRPC route should proxy");
    };
    assert_eq!(grpc_proxy.upstream_name, "servers[0]::dashboard_grpc");
    assert_eq!(grpc_proxy.upstream.protocol, rginx_core::UpstreamProtocol::H2c);
    assert!(grpc_proxy.preserve_host);
    assert!(grpc_proxy.proxy_set_headers.iter().any(|(name, value)| {
        name.as_str() == "nz-realip" && matches!(value, rginx_core::ProxyHeaderValue::ClientIp)
    }));

    let ws_route = vhost
        .routes
        .iter()
        .find(|route| route.id.contains("regex:i:^/api/v1/ws/(server|terminal|file)(/.*)?$"))
        .expect("WebSocket regex route should compile");
    assert!(ws_route.matcher.matches("/API/v1/ws/server/session"));
    assert!(ws_route.matcher.matches("/api/v1/ws/terminal"));
    assert!(ws_route.matcher.matches("/api/v1/ws/file/abc"));
    assert!(!ws_route.matcher.matches("/api/v1/ws/server-extra"));
    assert!(!ws_route.matcher.matches("/api/v1/ws/metrics"));
    assert_eq!(ws_route.response_buffering, rginx_core::RouteBufferingPolicy::Off);
    assert_eq!(ws_route.compression, rginx_core::RouteCompressionPolicy::Off);
    let rginx_core::RouteAction::Proxy(ws_proxy) = &ws_route.action else {
        panic!("WebSocket route should proxy");
    };
    assert_eq!(ws_proxy.upstream_name, "servers[0]::dashboard_http");
    assert!(ws_proxy.proxy_set_headers.iter().any(|(name, value)| {
        name.as_str() == "origin"
            && matches!(value, rginx_core::ProxyHeaderValue::Template(template) if template.as_str() == "https://{host}")
    }));

    let fallback_route = vhost
        .routes
        .iter()
        .find(|route| matches!(route.matcher, rginx_core::RouteMatcher::Prefix(ref path) if path == "/"))
        .expect("web fallback route should compile");
    let rginx_core::RouteAction::Proxy(fallback_proxy) = &fallback_route.action else {
        panic!("fallback route should proxy");
    };
    assert_eq!(fallback_proxy.upstream_name, "servers[0]::dashboard_http");
}
