use super::*;

#[test]
fn compile_http3_listener_defaults_to_tcp_listen_addr_and_default_alt_svc_policy() {
    let base_dir = temp_base_dir("rginx-http3-compile-test");
    let cert_path = base_dir.path().join("server.crt");
    let key_path = base_dir.path().join("server.key");
    fs::write(&cert_path, "placeholder cert").expect("cert file should be written");
    fs::write(&key_path, "placeholder key").expect("key file should be written");

    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8443".to_string()),
            server_header: None,
            proxy_protocol: None,
            default_certificate: None,
            server_names: vec!["localhost".to_string()],
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
            tls: Some(ServerTlsConfig {
                cert_path: cert_path.display().to_string(),
                key_path: key_path.display().to_string(),
                additional_certificates: None,
                versions: None,
                cipher_suites: None,
                key_exchange_groups: None,
                alpn_protocols: None,
                ocsp_staple_path: None,
                ocsp: None,
                session_resumption: None,
                session_tickets: None,
                session_cache_size: None,
                session_ticket_count: None,
                client_auth: None,
            }),
            http3: Some(Http3Config {
                listen: None,
                advertise_alt_svc: None,
                alt_svc_max_age_secs: None,
                max_concurrent_streams: None,
                stream_buffer_size_bytes: None,
                active_connection_id_limit: None,
                retry: None,
                host_key_path: None,
                gso: None,
                early_data: None,
            }),
        },
        upstreams: Vec::new(),
        locations: vec![LocationConfig {
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

    let snapshot = compile_with_base(config, base_dir.path()).expect("http3 config should compile");
    let listener = snapshot.listeners.first().expect("snapshot should have one listener");
    let http3 = listener.http3.as_ref().expect("listener should compile http3 metadata");
    assert_eq!(http3.listen_addr, "127.0.0.1:8443".parse().unwrap());
    assert!(http3.advertise_alt_svc);
    assert_eq!(http3.alt_svc_max_age.as_secs(), 86_400);
    assert_eq!(http3.max_concurrent_streams, super::server::DEFAULT_HTTP3_MAX_CONCURRENT_STREAMS);
    assert_eq!(http3.stream_buffer_size, super::server::DEFAULT_HTTP3_STREAM_BUFFER_SIZE_BYTES);
    assert_eq!(
        http3.active_connection_id_limit,
        super::server::DEFAULT_HTTP3_ACTIVE_CONNECTION_ID_LIMIT
    );
    assert_eq!(http3.retry, super::server::DEFAULT_HTTP3_RETRY);
    assert_eq!(http3.host_key_path, None);
    assert_eq!(http3.gso, super::server::DEFAULT_HTTP3_GSO);
    assert!(!http3.early_data_enabled);
}

#[test]
fn compile_http3_applies_transport_settings_and_resolves_host_key_path() {
    let base_dir = temp_base_dir("rginx-compile-http3-transport");
    let cert_path = base_dir.path().join("server.crt");
    let key_path = base_dir.path().join("server.key");
    fs::write(&cert_path, b"placeholder").expect("server cert should be written");
    fs::write(&key_path, b"placeholder").expect("server key should be written");

    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 2,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8443".to_string()),
            server_header: None,
            proxy_protocol: None,
            default_certificate: None,
            server_names: vec!["localhost".to_string()],
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
            tls: Some(ServerTlsConfig {
                cert_path: "server.crt".to_string(),
                key_path: "server.key".to_string(),
                additional_certificates: None,
                versions: None,
                cipher_suites: None,
                key_exchange_groups: None,
                alpn_protocols: None,
                ocsp_staple_path: None,
                ocsp: None,
                session_resumption: None,
                session_tickets: None,
                session_cache_size: None,
                session_ticket_count: None,
                client_auth: None,
            }),
            http3: Some(Http3Config {
                listen: Some("127.0.0.1:9443".to_string()),
                advertise_alt_svc: Some(false),
                alt_svc_max_age_secs: Some(7200),
                max_concurrent_streams: Some(256),
                stream_buffer_size_bytes: Some(131072),
                active_connection_id_limit: Some(5),
                retry: Some(true),
                host_key_path: Some("quic/host.key".to_string()),
                gso: Some(true),
                early_data: Some(true),
            }),
        },
        upstreams: Vec::new(),
        locations: vec![LocationConfig {
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

    let snapshot =
        compile_with_base(config, base_dir.path()).expect("http3 transport config should compile");
    let http3 = snapshot.listeners[0].http3.as_ref().expect("http3 should compile");
    assert_eq!(http3.listen_addr, "127.0.0.1:9443".parse().unwrap());
    assert!(!http3.advertise_alt_svc);
    assert_eq!(http3.alt_svc_max_age.as_secs(), 7200);
    assert_eq!(http3.max_concurrent_streams, 256);
    assert_eq!(http3.stream_buffer_size, 131072);
    assert_eq!(http3.active_connection_id_limit, 5);
    assert!(http3.retry);
    assert_eq!(http3.host_key_path, Some(base_dir.path().join("quic/host.key")));
    assert!(http3.gso);
    assert!(http3.early_data_enabled);
}
