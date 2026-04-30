use super::*;

#[test]
fn compile_applies_custom_server_header() {
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
            server_header: Some("edge-rginx".to_string()),
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
        locations: vec![test_location(
            MatcherConfig::Exact("/".to_string()),
            HandlerConfig::Return {
                status: 200,
                location: String::new(),
                body: Some("ok\n".to_string()),
            },
        )],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("custom server_header should compile");
    assert_eq!(default_listener_server(&snapshot).server_header, "edge-rginx");
}

#[test]
fn compile_normalizes_trusted_proxy_ips_and_cidrs() {
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
            trusted_proxies: vec!["10.0.0.0/8".to_string(), "127.0.0.1".to_string()],
            client_ip_header: Some("CF-Connecting-IP".to_string()),
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

    let snapshot = compile(config).expect("trusted proxies should compile");
    assert_eq!(default_listener_server(&snapshot).trusted_proxies.len(), 2);
    assert_eq!(
        default_listener_server(&snapshot).client_ip_header.as_ref().map(|name| name.as_str()),
        Some("cf-connecting-ip")
    );
    assert!(default_listener_server(&snapshot).is_trusted_proxy("10.1.2.3".parse().unwrap()));
    assert!(default_listener_server(&snapshot).is_trusted_proxy("127.0.0.1".parse().unwrap()));
}

#[test]
fn compile_attaches_server_hardening_settings() {
    let config = Config {
        acme: None,
        cache_zones: Vec::new(),
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: Some(3),
            accept_workers: Some(2),
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
            keep_alive: Some(false),
            max_headers: Some(32),
            max_request_body_bytes: Some(1024),
            max_connections: Some(256),
            header_read_timeout_secs: Some(3),
            request_body_read_timeout_secs: Some(4),
            response_write_timeout_secs: Some(5),
            access_log_format: Some("$request_id $status $request".to_string()),
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

    let snapshot = compile(config).expect("server hardening settings should compile");
    assert_eq!(snapshot.runtime.worker_threads, Some(3));
    assert_eq!(snapshot.runtime.accept_workers, 2);
    assert!(!default_listener_server(&snapshot).keep_alive);
    assert_eq!(default_listener_server(&snapshot).max_headers, Some(32));
    assert_eq!(default_listener_server(&snapshot).max_request_body_bytes, Some(1024));
    assert_eq!(default_listener_server(&snapshot).max_connections, Some(256));
    assert_eq!(
        default_listener_server(&snapshot).header_read_timeout,
        Some(Duration::from_secs(3))
    );
    assert_eq!(
        default_listener_server(&snapshot).request_body_read_timeout,
        Some(Duration::from_secs(4))
    );
    assert_eq!(
        default_listener_server(&snapshot).response_write_timeout,
        Some(Duration::from_secs(5))
    );
    let access_log_format = default_listener_server(&snapshot)
        .access_log_format
        .as_ref()
        .expect("access log format should compile");
    assert_eq!(access_log_format.template(), "$request_id $status $request");
}

#[test]
fn compile_rejects_invalid_server_access_log_format() {
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
            access_log_format: Some("$trace_id $status".to_string()),
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

    let error = compile(config).expect_err("unknown access log variables should be rejected");
    assert!(error.to_string().contains("access_log_format variable `$trace_id`"));
}
