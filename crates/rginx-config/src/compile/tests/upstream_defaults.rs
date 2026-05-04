use super::*;

#[test]
fn compile_accepts_https_upstreams() {
    let config = Config {
        acme: None,
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
            name: "secure-backend".to_string(),
            peers: vec![UpstreamPeerConfig {
                url: "https://example.com".to_string(),
                weight: 1,
                backup: false,
                max_conns: None,
            }],
            tls: None,
            dns: None,
            protocol: UpstreamProtocolConfig::Auto,
            load_balance: UpstreamLoadBalanceConfig::IpHash,
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
            health_check_path: Some("/healthz".to_string()),
            health_check_grpc_service: None,
            health_check_interval_secs: None,
            health_check_timeout_secs: None,
            healthy_successes_required: None,
        }],
        locations: vec![test_location(
            MatcherConfig::Prefix("/api".to_string()),
            HandlerConfig::Proxy {
                upstream: "secure-backend".to_string(),
                preserve_host: None,
                strip_prefix: None,
                proxy_set_headers: std::collections::HashMap::new(),
            },
        )],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("https upstream should compile");
    assert_eq!(default_listener_server(&snapshot).server_header, "rginx");
    let proxy = match &snapshot.default_vhost.routes[0].action {
        rginx_core::RouteAction::Proxy(proxy) => proxy,
        _ => panic!("expected proxy route"),
    };

    let peer = proxy.upstream.next_peer().expect("expected one upstream peer");
    assert_eq!(proxy.upstream_name, "secure-backend");
    assert_eq!(peer.scheme, "https");
    assert_eq!(peer.authority, "example.com");
    assert_eq!(proxy.upstream.protocol, rginx_core::UpstreamProtocol::Auto);
    assert_eq!(proxy.upstream.load_balance, rginx_core::UpstreamLoadBalance::IpHash);
    assert_eq!(
        proxy.upstream.request_timeout,
        Duration::from_secs(DEFAULT_UPSTREAM_REQUEST_TIMEOUT_SECS)
    );
    assert_eq!(
        proxy.upstream.max_replayable_request_body_bytes,
        DEFAULT_MAX_REPLAYABLE_REQUEST_BODY_BYTES as usize
    );
    assert_eq!(proxy.upstream.unhealthy_after_failures, DEFAULT_UNHEALTHY_AFTER_FAILURES);
    assert_eq!(
        proxy.upstream.unhealthy_cooldown,
        Duration::from_secs(DEFAULT_UNHEALTHY_COOLDOWN_SECS)
    );
    let active_health = proxy
        .upstream
        .active_health_check
        .as_ref()
        .expect("active health-check config should compile");
    assert_eq!(active_health.path, "/healthz");
    assert_eq!(active_health.grpc_service, None);
    assert_eq!(active_health.interval, Duration::from_secs(DEFAULT_HEALTH_CHECK_INTERVAL_SECS));
    assert_eq!(active_health.timeout, Duration::from_secs(DEFAULT_HEALTH_CHECK_TIMEOUT_SECS));
    assert_eq!(active_health.healthy_successes_required, DEFAULT_HEALTHY_SUCCESSES_REQUIRED);
}

#[test]
fn compile_defaults_grpc_health_check_path_when_service_is_set() {
    let config = Config {
        acme: None,
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
            name: "grpc-backend".to_string(),
            peers: vec![UpstreamPeerConfig {
                url: "https://example.com".to_string(),
                weight: 1,
                backup: false,
                max_conns: None,
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
            health_check_grpc_service: Some("grpc.health.v1.Health".to_string()),
            health_check_interval_secs: None,
            health_check_timeout_secs: None,
            healthy_successes_required: None,
        }],
        locations: vec![test_location(
            MatcherConfig::Prefix("/".to_string()),
            HandlerConfig::Proxy {
                upstream: "grpc-backend".to_string(),
                preserve_host: None,
                strip_prefix: None,
                proxy_set_headers: std::collections::HashMap::new(),
            },
        )],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("gRPC health-check config should compile");
    let proxy = match &snapshot.default_vhost.routes[0].action {
        rginx_core::RouteAction::Proxy(proxy) => proxy,
        _ => panic!("expected proxy route"),
    };

    let active_health = proxy
        .upstream
        .active_health_check
        .as_ref()
        .expect("gRPC active health-check config should compile");
    assert_eq!(active_health.path, super::DEFAULT_GRPC_HEALTH_CHECK_PATH);
    assert_eq!(active_health.grpc_service.as_deref(), Some("grpc.health.v1.Health"));
}
