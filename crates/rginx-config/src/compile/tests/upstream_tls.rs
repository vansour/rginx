use super::*;

#[test]
fn compile_resolves_custom_ca_relative_to_config_base() {
    let base_dir = temp_base_dir("rginx-config-test-");
    let ca_path = base_dir.path().join("dev-ca.pem");
    fs::write(&ca_path, b"placeholder").expect("temp CA file should be written");

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
            name: "dev-backend".to_string(),
            peers: vec![UpstreamPeerConfig {
                url: "https://localhost:9443".to_string(),
                weight: 1,
                backup: false,
            }],
            tls: Some(UpstreamTlsConfig {
                verify: crate::model::UpstreamTlsModeConfig::CustomCa {
                    ca_cert_path: "dev-ca.pem".to_string(),
                },
                versions: Some(vec![crate::model::TlsVersionConfig::Tls13]),
                verify_depth: None,
                crl_path: None,
                client_cert_path: None,
                client_key_path: None,
            }),
            dns: None,
            protocol: UpstreamProtocolConfig::Http2,
            load_balance: UpstreamLoadBalanceConfig::RoundRobin,
            server_name: None,
            server_name_override: Some("dev.internal".to_string()),
            request_timeout_secs: Some(5),
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
            max_replayable_request_body_bytes: Some(1024),
            unhealthy_after_failures: Some(3),
            unhealthy_cooldown_secs: Some(15),
            health_check_path: Some("/ready".to_string()),
            health_check_grpc_service: None,
            health_check_interval_secs: Some(7),
            health_check_timeout_secs: Some(3),
            healthy_successes_required: Some(4),
        }],
        locations: vec![LocationConfig {
            cache: None,
            matcher: MatcherConfig::Prefix("/".to_string()),
            handler: HandlerConfig::Proxy {
                upstream: "dev-backend".to_string(),
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

    let snapshot =
        compile_with_base(config, base_dir.path()).expect("custom CA config should compile");
    let proxy = match &snapshot.default_vhost.routes[0].action {
        rginx_core::RouteAction::Proxy(proxy) => proxy,
        _ => panic!("expected proxy route"),
    };

    assert!(matches!(
        &proxy.upstream.tls,
        rginx_core::UpstreamTls::CustomCa { ca_cert_path } if ca_cert_path == &ca_path
    ));
    assert_eq!(proxy.upstream.protocol, rginx_core::UpstreamProtocol::Http2);
    assert_eq!(proxy.upstream.server_name_override.as_deref(), Some("dev.internal"));
    assert_eq!(proxy.upstream.request_timeout, Duration::from_secs(5));
    assert_eq!(proxy.upstream.max_replayable_request_body_bytes, 1024);
    assert_eq!(proxy.upstream.unhealthy_after_failures, 3);
    assert_eq!(proxy.upstream.unhealthy_cooldown, Duration::from_secs(15));
    let active_health = proxy
        .upstream
        .active_health_check
        .as_ref()
        .expect("custom active health-check config should compile");
    assert_eq!(active_health.path, "/ready");
    assert_eq!(active_health.grpc_service, None);
    assert_eq!(active_health.interval, Duration::from_secs(7));
    assert_eq!(active_health.timeout, Duration::from_secs(3));
    assert_eq!(active_health.healthy_successes_required, 4);
}

#[test]
fn compile_accepts_https_http3_upstreams() {
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
            name: "h3-backend".to_string(),
            peers: vec![UpstreamPeerConfig {
                url: "https://example.com:443".to_string(),
                weight: 1,
                backup: false,
            }],
            tls: None,
            dns: None,
            protocol: UpstreamProtocolConfig::Http3,
            load_balance: UpstreamLoadBalanceConfig::RoundRobin,
            server_name: None,
            server_name_override: Some("example.com".to_string()),
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
                upstream: "h3-backend".to_string(),
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

    let snapshot = compile(config).expect("http3 upstream should compile");
    let proxy = match &snapshot.default_vhost.routes[0].action {
        rginx_core::RouteAction::Proxy(proxy) => proxy,
        _ => panic!("expected proxy route"),
    };
    assert_eq!(proxy.upstream.protocol, rginx_core::UpstreamProtocol::Http3);
    assert_eq!(proxy.upstream.server_name_override.as_deref(), Some("example.com"));
}

#[test]
fn compile_resolves_upstream_mtls_identity_and_tls_versions_relative_to_config_base() {
    let base_dir = temp_base_dir("rginx-upstream-mtls-config-test-");
    let ca_path = base_dir.path().join("upstream-ca.pem");
    let client_cert_path = base_dir.path().join("client.crt");
    let client_key_path = base_dir.path().join("client.key");
    fs::write(&ca_path, b"placeholder").expect("temp CA file should be written");
    fs::write(&client_cert_path, b"placeholder").expect("temp client cert file should be written");
    fs::write(&client_key_path, b"placeholder").expect("temp client key file should be written");

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
            name: "mtls-backend".to_string(),
            peers: vec![UpstreamPeerConfig {
                url: "https://localhost:9443".to_string(),
                weight: 1,
                backup: false,
            }],
            tls: Some(UpstreamTlsConfig {
                verify: crate::model::UpstreamTlsModeConfig::CustomCa {
                    ca_cert_path: "upstream-ca.pem".to_string(),
                },
                versions: Some(vec![crate::model::TlsVersionConfig::Tls13]),
                verify_depth: None,
                crl_path: None,
                client_cert_path: Some("client.crt".to_string()),
                client_key_path: Some("client.key".to_string()),
            }),
            dns: None,
            protocol: UpstreamProtocolConfig::Auto,
            load_balance: UpstreamLoadBalanceConfig::RoundRobin,
            server_name: None,
            server_name_override: Some("localhost".to_string()),
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
                upstream: "mtls-backend".to_string(),
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

    let snapshot =
        compile_with_base(config, base_dir.path()).expect("upstream mTLS config should compile");
    let proxy = match &snapshot.default_vhost.routes[0].action {
        rginx_core::RouteAction::Proxy(proxy) => proxy,
        _ => panic!("expected proxy route"),
    };

    assert!(matches!(
        &proxy.upstream.tls,
        rginx_core::UpstreamTls::CustomCa { ca_cert_path } if ca_cert_path == &ca_path
    ));
    assert!(matches!(
        proxy.upstream.tls_versions.as_deref(),
        Some([rginx_core::TlsVersion::Tls13])
    ));
    let client_identity =
        proxy.upstream.client_identity.as_ref().expect("client identity should compile");
    assert_eq!(client_identity.cert_path, client_cert_path);
    assert_eq!(client_identity.key_path, client_key_path);
}
