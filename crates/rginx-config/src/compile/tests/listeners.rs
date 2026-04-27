use super::*;

#[test]
fn compile_supports_explicit_multi_listener_configs() {
    let config = Config {
        cache_zones: Vec::new(),
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: vec![
            ListenerConfig {
                name: "http".to_string(),
                server_header: None,
                proxy_protocol: None,
                default_certificate: None,
                listen: "127.0.0.1:8080".to_string(),
                trusted_proxies: Vec::new(),
                client_ip_header: None,
                keep_alive: Some(true),
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: Some(10),
                header_read_timeout_secs: None,
                request_body_read_timeout_secs: None,
                response_write_timeout_secs: None,
                access_log_format: None,
                tls: None,
                http3: None,
            },
            ListenerConfig {
                name: "https".to_string(),
                server_header: None,
                proxy_protocol: None,
                default_certificate: None,
                listen: "127.0.0.1:8443".to_string(),
                trusted_proxies: Vec::new(),
                client_ip_header: None,
                keep_alive: Some(true),
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: Some(20),
                header_read_timeout_secs: None,
                request_body_read_timeout_secs: None,
                response_write_timeout_secs: None,
                access_log_format: None,
                tls: None,
                http3: None,
            },
        ],
        server: ServerConfig {
            listen: None,
            server_header: None,
            proxy_protocol: None,
            default_certificate: None,
            server_names: vec!["example.com".to_string()],
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

    let snapshot = compile(config).expect("explicit multi-listener config should compile");
    assert_eq!(snapshot.listeners.len(), 2);
    assert_eq!(snapshot.total_listener_count(), 2);
    assert_eq!(snapshot.listeners[0].server.listen_addr, "127.0.0.1:8080".parse().unwrap());
    assert_eq!(snapshot.listeners[0].name, "http");
    assert_eq!(snapshot.listeners[1].name, "https");
    assert_eq!(snapshot.listeners[1].server.max_connections, Some(20));
}
