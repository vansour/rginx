use super::*;

#[test]
fn compile_applies_granular_upstream_transport_settings() {
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
            name: "backend".to_string(),
            peers: vec![UpstreamPeerConfig {
                url: "https://example.com".to_string(),
                weight: 1,
                backup: false,
            }],
            tls: None,
            dns: None,
            protocol: UpstreamProtocolConfig::Auto,
            load_balance: UpstreamLoadBalanceConfig::IpHash,
            server_name: None,
            server_name_override: None,
            request_timeout_secs: None,
            connect_timeout_secs: Some(3),
            read_timeout_secs: Some(4),
            write_timeout_secs: Some(5),
            idle_timeout_secs: Some(6),
            pool_idle_timeout_secs: Some(7),
            pool_max_idle_per_host: Some(8),
            tcp_keepalive_secs: Some(9),
            tcp_nodelay: Some(true),
            http2_keep_alive_interval_secs: Some(10),
            http2_keep_alive_timeout_secs: Some(11),
            http2_keep_alive_while_idle: Some(true),
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
            MatcherConfig::Prefix("/".to_string()),
            HandlerConfig::Proxy {
                upstream: "backend".to_string(),
                preserve_host: None,
                strip_prefix: None,
                proxy_set_headers: std::collections::HashMap::new(),
            },
        )],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("granular upstream settings should compile");
    let proxy = match &snapshot.default_vhost.routes[0].action {
        rginx_core::RouteAction::Proxy(proxy) => proxy,
        _ => panic!("expected proxy route"),
    };

    assert_eq!(proxy.upstream.load_balance, rginx_core::UpstreamLoadBalance::IpHash);
    assert_eq!(proxy.upstream.connect_timeout, Duration::from_secs(3));
    assert_eq!(proxy.upstream.request_timeout, Duration::from_secs(4));
    assert_eq!(proxy.upstream.write_timeout, Duration::from_secs(5));
    assert_eq!(proxy.upstream.idle_timeout, Duration::from_secs(6));
    assert_eq!(proxy.upstream.pool_idle_timeout, Some(Duration::from_secs(7)));
    assert_eq!(proxy.upstream.pool_max_idle_per_host, 8);
    assert_eq!(proxy.upstream.tcp_keepalive, Some(Duration::from_secs(9)));
    assert!(proxy.upstream.tcp_nodelay);
    assert_eq!(proxy.upstream.http2_keep_alive_interval, Some(Duration::from_secs(10)));
    assert_eq!(proxy.upstream.http2_keep_alive_timeout, Duration::from_secs(11));
    assert!(proxy.upstream.http2_keep_alive_while_idle);
}

#[test]
fn compile_accepts_least_conn_load_balance() {
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
            name: "backend".to_string(),
            peers: vec![
                UpstreamPeerConfig {
                    url: "http://127.0.0.1:9000".to_string(),
                    weight: 1,
                    backup: false,
                },
                UpstreamPeerConfig {
                    url: "http://127.0.0.1:9001".to_string(),
                    weight: 1,
                    backup: false,
                },
            ],
            tls: None,
            dns: None,
            protocol: UpstreamProtocolConfig::Auto,
            load_balance: UpstreamLoadBalanceConfig::LeastConn,
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
        locations: vec![LocationConfig {
            cache: None,
            matcher: MatcherConfig::Prefix("/".to_string()),
            handler: HandlerConfig::Proxy {
                upstream: "backend".to_string(),
                preserve_host: None,
                strip_prefix: None,
                proxy_set_headers: std::collections::HashMap::new(),
            },
            grpc_service: None,

            grpc_method: None,

            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
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

    let snapshot = compile(config).expect("least_conn config should compile");
    let proxy = match &snapshot.default_vhost.routes[0].action {
        rginx_core::RouteAction::Proxy(proxy) => proxy,
        _ => panic!("expected proxy route"),
    };

    assert_eq!(proxy.upstream.load_balance, rginx_core::UpstreamLoadBalance::LeastConn);
}

#[test]
fn compile_applies_peer_weights() {
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
            name: "backend".to_string(),
            peers: vec![
                UpstreamPeerConfig {
                    url: "http://127.0.0.1:9000".to_string(),
                    weight: 3,
                    backup: false,
                },
                UpstreamPeerConfig {
                    url: "http://127.0.0.1:9001".to_string(),
                    weight: 1,
                    backup: false,
                },
            ],
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
        locations: vec![LocationConfig {
            cache: None,
            matcher: MatcherConfig::Prefix("/".to_string()),
            handler: HandlerConfig::Proxy {
                upstream: "backend".to_string(),
                preserve_host: None,
                strip_prefix: None,
                proxy_set_headers: std::collections::HashMap::new(),
            },
            grpc_service: None,

            grpc_method: None,

            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
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

    let snapshot = compile(config).expect("weighted peer config should compile");
    let proxy = match &snapshot.default_vhost.routes[0].action {
        rginx_core::RouteAction::Proxy(proxy) => proxy,
        _ => panic!("expected proxy route"),
    };

    assert_eq!(proxy.upstream.peers[0].weight, 3);
    assert_eq!(proxy.upstream.peers[1].weight, 1);

    let observed = (0..4)
        .map(|_| proxy.upstream.next_peer().expect("expected weighted peer").url.clone())
        .collect::<Vec<_>>();

    assert_eq!(
        observed,
        vec![
            "http://127.0.0.1:9000".to_string(),
            "http://127.0.0.1:9000".to_string(),
            "http://127.0.0.1:9000".to_string(),
            "http://127.0.0.1:9001".to_string(),
        ]
    );
}
