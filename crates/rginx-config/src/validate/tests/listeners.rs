use super::*;

#[test]
fn validate_accepts_explicit_listeners_when_legacy_listener_fields_are_empty() {
    let mut config = base_config();
    config.server.listen = None;
    config.listeners = vec![
        ListenerConfig {
            name: "http".to_string(),
            server_header: None,
            proxy_protocol: None,
            default_certificate: None,
            listen: "127.0.0.1:8080".to_string(),
            trusted_proxies: Vec::new(),
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
            keep_alive: Some(true),
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: Some(20),
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
    ];

    validate(&config).expect("explicit listeners should validate");
}

#[test]
fn validate_accepts_request_buffering_on_with_legacy_body_limit() {
    let mut config = base_config();
    config.server.max_request_body_bytes = Some(1024);
    config.locations[0].request_buffering = Some(RouteBufferingPolicyConfig::On);

    validate(&config).expect("request_buffering=On should validate with a legacy body limit");
}

#[test]
fn validate_rejects_request_buffering_on_without_legacy_body_limit() {
    let mut config = base_config();
    config.locations[0].request_buffering = Some(RouteBufferingPolicyConfig::On);

    let error =
        validate(&config).expect_err("request_buffering=On should require a legacy body limit");
    assert!(
        error.to_string().contains("request_buffering=On requires server.max_request_body_bytes")
    );
}

#[test]
fn validate_rejects_request_buffering_on_without_explicit_listener_body_limits() {
    let mut config = base_config();
    config.server.listen = None;
    config.locations[0].request_buffering = Some(RouteBufferingPolicyConfig::On);
    config.listeners = vec![
        ListenerConfig {
            name: "http".to_string(),
            server_header: None,
            proxy_protocol: None,
            default_certificate: None,
            listen: "127.0.0.1:8080".to_string(),
            trusted_proxies: Vec::new(),
            keep_alive: Some(true),
            max_headers: None,
            max_request_body_bytes: Some(1024),
            max_connections: None,
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
            keep_alive: Some(true),
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: Some(valid_server_tls()),
            http3: None,
        },
    ];

    let error = validate(&config)
        .expect_err("request_buffering=On should require limits on all explicit listeners");
    assert!(error.to_string().contains(
        "request_buffering=On requires max_request_body_bytes on every explicit listener"
    ));
    assert!(error.to_string().contains("https"));
}

#[test]
fn validate_rejects_duplicate_listener_names_after_ascii_normalization() {
    let mut config = base_config();
    config.server.listen = None;
    config.listeners = vec![
        ListenerConfig {
            name: " HTTP ".to_string(),
            server_header: None,
            proxy_protocol: None,
            default_certificate: None,
            listen: "127.0.0.1:8080".to_string(),
            trusted_proxies: Vec::new(),
            keep_alive: Some(true),
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
        ListenerConfig {
            name: "http".to_string(),
            server_header: None,
            proxy_protocol: None,
            default_certificate: None,
            listen: "127.0.0.1:8443".to_string(),
            trusted_proxies: Vec::new(),
            keep_alive: Some(true),
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
    ];

    let error = validate(&config).expect_err("normalized duplicate listener names should fail");
    assert!(error.to_string().contains("duplicate listener name `http` across listeners"));
}

#[test]
fn validate_rejects_mixing_legacy_listener_fields_with_explicit_listeners() {
    let mut config = base_config();
    config.listeners = vec![ListenerConfig {
        name: "http".to_string(),
        server_header: None,
        proxy_protocol: None,
        default_certificate: None,
        listen: "127.0.0.1:8080".to_string(),
        trusted_proxies: Vec::new(),
        keep_alive: Some(true),
        max_headers: None,
        max_request_body_bytes: None,
        max_connections: None,
        header_read_timeout_secs: None,
        request_body_read_timeout_secs: None,
        response_write_timeout_secs: None,
        access_log_format: None,
        tls: None,
        http3: None,
    }];

    let error = validate(&config).expect_err("mixed legacy and explicit listeners should fail");
    assert!(error.to_string().contains("cannot be used together with listeners"));
}
