use super::*;

#[test]
fn compile_attaches_regex_matcher_and_dynamic_proxy_headers() {
    let config = Config {
        cache_zones: Vec::new(),
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            server_header: None,
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            client_ip_header: None,
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
            http3: None,
        },
        upstreams: vec![UpstreamConfig {
            name: "dashboard".to_string(),
            peers: vec![UpstreamPeerConfig {
                url: "http://127.0.0.1:8008".to_string(),
                weight: 1,
                backup: false,
            }],
            tls: None,
            dns: None,
            protocol: UpstreamProtocolConfig::Auto,
            load_balance: UpstreamLoadBalanceConfig::RoundRobin,
            server_name: None,
            server_name_override: None,
            request_timeout_secs: None,
            connect_timeout_secs: None,
            read_timeout_secs: None,
            write_timeout_secs: None,
            idle_timeout_secs: None,
            pool_idle_timeout_secs: None,
            pool_max_idle_per_host: None,
            tcp_keepalive_secs: None,
            tcp_nodelay: None,
            http2_keep_alive_interval_secs: None,
            http2_keep_alive_timeout_secs: None,
            http2_keep_alive_while_idle: None,
            max_replayable_request_body_bytes: None,
            unhealthy_after_failures: None,
            unhealthy_cooldown_secs: None,
            health_check_path: None,
            health_check_grpc_service: None,
            health_check_interval_secs: None,
            health_check_timeout_secs: None,
            healthy_successes_required: None,
        }],
        locations: vec![test_location(
            MatcherConfig::Regex {
                pattern: "^/api/v1/ws/(server|terminal|file)(/.*)?$".to_string(),
                case_insensitive: true,
            },
            HandlerConfig::Proxy {
                upstream: "dashboard".to_string(),
                preserve_host: Some(true),
                strip_prefix: None,
                proxy_set_headers: std::collections::HashMap::from([
                    (
                        "nz-realip".to_string(),
                        ProxyHeaderValueConfig::Dynamic(ProxyHeaderDynamicValueConfig::ClientIp),
                    ),
                    (
                        "origin".to_string(),
                        ProxyHeaderValueConfig::Dynamic(ProxyHeaderDynamicValueConfig::Template(
                            "https://{host}".to_string(),
                        )),
                    ),
                ]),
            },
        )],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("regex proxy route should compile");
    let route = &snapshot.default_vhost.routes[0];
    assert!(route.matcher.matches("/API/V1/WS/server"));
    assert!(!route.matcher.matches("/api/v1/ws/metrics"));
    assert!(route.id.contains("regex:i:^/api/v1/ws/(server|terminal|file)(/.*)?$"));
    let rginx_core::RouteAction::Proxy(proxy) = &route.action else {
        panic!("route should proxy");
    };
    assert_eq!(proxy.proxy_set_headers.len(), 2);
    let nz_realip = proxy
        .proxy_set_headers
        .iter()
        .find(|(name, _)| name.as_str() == "nz-realip")
        .map(|(_, value)| value)
        .expect("nz-realip header should be compiled");
    assert!(matches!(nz_realip, rginx_core::ProxyHeaderValue::ClientIp));
    let origin = proxy
        .proxy_set_headers
        .iter()
        .find(|(name, _)| name.as_str() == "origin")
        .map(|(_, value)| value)
        .expect("origin header should be compiled");
    assert!(
        matches!(origin, rginx_core::ProxyHeaderValue::Template(template) if template.as_str() == "https://{host}")
    );
}

#[test]
fn compile_preserves_declaration_order_for_overlapping_regex_routes() {
    let config = Config {
        cache_zones: Vec::new(),
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            server_header: None,
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            client_ip_header: None,
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
            http3: None,
        },
        upstreams: Vec::new(),
        locations: vec![
            test_location(
                MatcherConfig::Regex { pattern: "^/api/.*$".to_string(), case_insensitive: false },
                HandlerConfig::Return {
                    status: 200,
                    location: String::new(),
                    body: Some("broad\n".to_string()),
                },
            ),
            test_location(
                MatcherConfig::Regex {
                    pattern: "^/api/v1/longer/.*$".to_string(),
                    case_insensitive: false,
                },
                HandlerConfig::Return {
                    status: 200,
                    location: String::new(),
                    body: Some("narrow\n".to_string()),
                },
            ),
        ],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("overlapping regex routes should compile");
    assert_eq!(snapshot.default_vhost.routes[0].id, "server/routes[0]|regex:^/api/.*$");
    assert_eq!(snapshot.default_vhost.routes[1].id, "server/routes[1]|regex:^/api/v1/longer/.*$");
}
