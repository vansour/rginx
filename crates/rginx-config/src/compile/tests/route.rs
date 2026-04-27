use super::*;

mod regex;

#[test]
fn compile_attaches_route_access_control() {
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
        locations: vec![LocationConfig {
            cache: None,
            matcher: MatcherConfig::Exact("/".to_string()),
            handler: HandlerConfig::Return {
                status: 200,
                location: String::new(),
                body: Some("ok\n".to_string()),
            },
            grpc_service: None,

            grpc_method: None,

            allow_cidrs: vec!["127.0.0.1/32".to_string(), "::1/128".to_string()],
            deny_cidrs: vec!["127.0.0.2/32".to_string()],
            requests_per_sec: None,
            burst: None,
            allow_early_data: None,
            request_buffering: None,
            response_buffering: None,
            compression: None,
            compression_min_bytes: None,
            compression_content_types: None,
            streaming_response_idle_timeout_secs: None,
        }],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("access-controlled route should compile");
    assert_eq!(snapshot.default_vhost.routes[0].access_control.allow_cidrs.len(), 2);
    assert_eq!(snapshot.default_vhost.routes[0].access_control.deny_cidrs.len(), 1);
}

#[test]
fn compile_attaches_route_rate_limit() {
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
        locations: vec![LocationConfig {
            cache: None,
            matcher: MatcherConfig::Prefix("/api".to_string()),
            handler: HandlerConfig::Return {
                status: 200,
                location: String::new(),
                body: Some("ok\n".to_string()),
            },
            grpc_service: None,

            grpc_method: None,

            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: Some(20),
            burst: Some(5),
            allow_early_data: None,
            request_buffering: None,
            response_buffering: None,
            compression: None,
            compression_min_bytes: None,
            compression_content_types: None,
            streaming_response_idle_timeout_secs: None,
        }],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("rate-limited route should compile");
    let rate_limit =
        snapshot.default_vhost.routes[0].rate_limit.expect("route rate limit should exist");
    assert_eq!(rate_limit.requests_per_sec, 20);
    assert_eq!(rate_limit.burst, 5);
}

#[test]
fn compile_applies_route_transport_policy_defaults_and_overrides() {
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
                MatcherConfig::Exact("/default".to_string()),
                HandlerConfig::Return {
                    status: 200,
                    location: String::new(),
                    body: Some("default\n".to_string()),
                },
            ),
            LocationConfig {
                cache: None,
                matcher: MatcherConfig::Exact("/custom".to_string()),
                handler: HandlerConfig::Return {
                    status: 200,
                    location: String::new(),
                    body: Some("custom\n".to_string()),
                },
                grpc_service: None,
                grpc_method: None,
                allow_cidrs: Vec::new(),
                deny_cidrs: Vec::new(),
                requests_per_sec: None,
                burst: None,
                allow_early_data: None,
                request_buffering: Some(RouteBufferingPolicyConfig::Off),
                response_buffering: Some(RouteBufferingPolicyConfig::On),
                compression: Some(RouteCompressionPolicyConfig::Force),
                compression_min_bytes: Some(1024),
                compression_content_types: Some(vec![
                    " text/plain ".to_string(),
                    "application/json".to_string(),
                ]),
                streaming_response_idle_timeout_secs: Some(15),
            },
        ],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("route transport policies should compile");
    let default_route = snapshot
        .default_vhost
        .routes
        .iter()
        .find(|route| matches!(route.matcher, rginx_core::RouteMatcher::Exact(ref path) if path == "/default"))
        .expect("default route should exist");
    assert_eq!(default_route.request_buffering, rginx_core::RouteBufferingPolicy::Auto);
    assert_eq!(default_route.response_buffering, rginx_core::RouteBufferingPolicy::Auto);
    assert_eq!(default_route.compression, rginx_core::RouteCompressionPolicy::Auto);
    assert_eq!(default_route.compression_min_bytes, None);
    assert!(default_route.compression_content_types.is_empty());
    assert_eq!(default_route.streaming_response_idle_timeout, None);

    let custom_route = snapshot
        .default_vhost
        .routes
        .iter()
        .find(|route| matches!(route.matcher, rginx_core::RouteMatcher::Exact(ref path) if path == "/custom"))
        .expect("custom route should exist");
    assert_eq!(custom_route.request_buffering, rginx_core::RouteBufferingPolicy::Off);
    assert_eq!(custom_route.response_buffering, rginx_core::RouteBufferingPolicy::On);
    assert_eq!(custom_route.compression, rginx_core::RouteCompressionPolicy::Force);
    assert_eq!(custom_route.compression_min_bytes, Some(1024));
    assert_eq!(
        custom_route.compression_content_types,
        vec!["text/plain".to_string(), "application/json".to_string()]
    );
    assert_eq!(custom_route.streaming_response_idle_timeout, Some(Duration::from_secs(15)));
}

#[test]
fn compile_generates_distinct_route_and_vhost_ids() {
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
            server_names: vec!["default.example.com".to_string()],
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
        locations: vec![test_location(
            MatcherConfig::Exact("/".to_string()),
            HandlerConfig::Return {
                status: 200,
                location: String::new(),
                body: Some("default site\n".to_string()),
            },
        )],
        servers: vec![VirtualHostConfig {
            listen: Vec::new(),
            server_names: vec!["api.example.com".to_string()],
            upstreams: Vec::new(),
            locations: vec![test_location(
                MatcherConfig::Exact("/".to_string()),
                HandlerConfig::Return {
                    status: 200,
                    location: String::new(),
                    body: Some("api site\n".to_string()),
                },
            )],
            tls: None,
            http3: None,
        }],
    };

    let snapshot = compile(config).expect("vhost config should compile");

    assert_eq!(snapshot.default_vhost.id, "server");
    assert_eq!(snapshot.vhosts[0].id, "servers[0]");
    assert_eq!(snapshot.default_vhost.routes[0].id, "server/routes[0]|exact:/");
    assert_eq!(snapshot.vhosts[0].routes[0].id, "servers[0]/routes[0]|exact:/");
    assert_eq!(snapshot.total_vhost_count(), 2);
    assert_eq!(snapshot.total_route_count(), 2);
}

#[test]
fn compile_prioritizes_grpc_constrained_routes_with_same_path_matcher() {
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
                MatcherConfig::Prefix("/".to_string()),
                HandlerConfig::Return {
                    status: 200,
                    location: String::new(),
                    body: Some("fallback\n".to_string()),
                },
            ),
            LocationConfig {
                cache: None,
                grpc_service: Some("grpc.health.v1.Health".to_string()),
                grpc_method: Some("Check".to_string()),
                ..test_location(
                    MatcherConfig::Prefix("/".to_string()),
                    HandlerConfig::Return {
                        status: 200,
                        location: String::new(),
                        body: Some("grpc\n".to_string()),
                    },
                )
            },
        ],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("gRPC route constraints should compile");
    let routes = &snapshot.default_vhost.routes;
    assert_eq!(routes.len(), 2);
    assert_eq!(
        routes[0].grpc_match.as_ref().and_then(|grpc| grpc.service.as_deref()),
        Some("grpc.health.v1.Health")
    );
    assert_eq!(
        routes[0].grpc_match.as_ref().and_then(|grpc| grpc.method.as_deref()),
        Some("Check")
    );
    assert!(routes[0].id.contains("grpc:service=grpc.health.v1.Health,method=Check"));
    assert!(routes[1].grpc_match.is_none());
}
