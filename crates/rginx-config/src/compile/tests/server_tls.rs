use super::*;

#[test]
fn compile_resolves_server_tls_paths_relative_to_config_base() {
    let base_dir = temp_base_dir("rginx-server-tls-config-test-");
    let cert_path = base_dir.path().join("server.crt");
    let key_path = base_dir.path().join("server.key");
    fs::write(&cert_path, b"placeholder").expect("temp cert file should be written");
    fs::write(&key_path, b"placeholder").expect("temp key file should be written");

    let config = Config {
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
            http3: None,
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

    let snapshot = compile_with_base(config, base_dir.path()).expect("server TLS should compile");
    let tls =
        default_listener_server(&snapshot).tls.clone().expect("compiled server TLS should exist");
    assert_eq!(tls.cert_path, cert_path);
    assert_eq!(tls.key_path, key_path);
}

#[test]
fn compile_preserves_server_tls_policy_fields() {
    let base_dir = temp_base_dir("rginx-server-tls-policy-test-");
    let cert_path = base_dir.path().join("server.crt");
    let key_path = base_dir.path().join("server.key");
    fs::write(&cert_path, b"placeholder").expect("temp cert file should be written");
    fs::write(&key_path, b"placeholder").expect("temp key file should be written");

    let config = Config {
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
                versions: Some(vec![crate::model::TlsVersionConfig::Tls13]),
                cipher_suites: Some(vec![TlsCipherSuiteConfig::Tls13Aes128GcmSha256]),
                key_exchange_groups: Some(vec![TlsKeyExchangeGroupConfig::Secp256r1]),
                alpn_protocols: Some(vec!["http/1.1".to_string()]),
                ocsp_staple_path: None,
                ocsp: None,
                session_resumption: Some(true),
                session_tickets: Some(false),
                session_cache_size: Some(512),
                session_ticket_count: None,
                client_auth: None,
            }),
            http3: None,
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

    let snapshot = compile_with_base(config, base_dir.path()).expect("server TLS should compile");
    let tls =
        default_listener_server(&snapshot).tls.clone().expect("compiled server TLS should exist");
    assert_eq!(tls.versions, Some(vec![rginx_core::TlsVersion::Tls13]));
    assert_eq!(tls.cipher_suites, Some(vec![rginx_core::TlsCipherSuite::Tls13Aes128GcmSha256]));
    assert_eq!(tls.key_exchange_groups, Some(vec![rginx_core::TlsKeyExchangeGroup::Secp256r1]));
    assert_eq!(tls.alpn_protocols, Some(vec!["http/1.1".to_string()]));
    assert_eq!(tls.session_resumption, Some(true));
    assert_eq!(tls.session_tickets, Some(false));
    assert_eq!(tls.session_cache_size, Some(512));
    assert_eq!(tls.session_ticket_count, None);
}

#[test]
fn compile_preserves_server_tls_ocsp_policy_fields() {
    let base_dir = temp_base_dir("rginx-server-ocsp-policy-test-");
    let cert_path = base_dir.path().join("server.crt");
    let key_path = base_dir.path().join("server.key");
    let ocsp_path = base_dir.path().join("server.ocsp");
    fs::write(&cert_path, b"placeholder").expect("temp cert file should be written");
    fs::write(&key_path, b"placeholder").expect("temp key file should be written");
    fs::write(&ocsp_path, b"").expect("temp ocsp file should be written");

    let config = Config {
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
                ocsp_staple_path: Some("server.ocsp".to_string()),
                ocsp: Some(crate::model::OcspConfig {
                    nonce: Some(crate::model::OcspNonceModeConfig::Required),
                    responder_policy: Some(crate::model::OcspResponderPolicyConfig::IssuerOnly),
                }),
                session_resumption: None,
                session_tickets: None,
                session_cache_size: None,
                session_ticket_count: None,
                client_auth: None,
            }),
            http3: None,
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

    let snapshot = compile_with_base(config, base_dir.path()).expect("server TLS should compile");
    let tls =
        default_listener_server(&snapshot).tls.clone().expect("compiled server TLS should exist");
    assert_eq!(tls.ocsp.nonce, rginx_core::OcspNonceMode::Required);
    assert_eq!(tls.ocsp.responder_policy, rginx_core::OcspResponderPolicy::IssuerOnly);
}
