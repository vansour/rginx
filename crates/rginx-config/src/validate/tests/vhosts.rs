use super::*;

#[test]
fn validate_rejects_empty_default_server_name() {
    let mut config = base_config();
    config.server.server_names = vec![" ".to_string()];

    let error = validate(&config).expect_err("empty default server_name should be rejected");
    assert!(error.to_string().contains("server server_name must not be empty"));
}

#[test]
fn validate_rejects_default_server_name_with_path_separator() {
    let mut config = base_config();
    config.server.server_names = vec!["api/example.com".to_string()];

    let error = validate(&config).expect_err("invalid default server_name should be rejected");
    assert!(
        error
            .to_string()
            .contains("server server_name `api/example.com` should not contain path separator")
    );
}

#[test]
fn validate_rejects_unsupported_server_name_wildcard_syntax() {
    let mut config = base_config();
    config.server.server_names = vec!["api.*.example.com".to_string()];

    let error = validate(&config).expect_err("unsupported wildcard syntax should be rejected");
    assert!(error.to_string().contains("unsupported wildcard syntax"));
}

#[test]
fn validate_rejects_duplicate_server_name_between_default_server_and_vhost() {
    let mut config = base_config();
    config.server.server_names = vec!["api.example.com".to_string()];
    config.servers = vec![sample_vhost(vec!["API.EXAMPLE.COM"])];

    let error = validate(&config).expect_err("duplicate server_names should be rejected");
    assert!(
        error
            .to_string()
            .contains("duplicate server_name `API.EXAMPLE.COM` across server and servers")
    );
}

#[test]
fn validate_rejects_vhost_without_server_name() {
    let mut config = base_config();
    config.servers = vec![sample_vhost(Vec::new())];

    let error = validate(&config).expect_err("vhost without server_name should be rejected");
    assert!(error.to_string().contains("servers[0] must define at least one server_name"));
}

#[test]
fn validate_rejects_tls_vhost_without_server_name() {
    let mut config = base_config();
    let mut vhost = sample_vhost(Vec::new());
    vhost.tls = Some(VirtualHostTlsConfig {
        cert_path: "server.crt".to_string(),
        key_path: "server.key".to_string(),
        additional_certificates: None,
        ocsp_staple_path: None,
        ocsp: None,
    });
    config.servers = vec![vhost];

    let error = validate(&config).expect_err("TLS vhost without server_name should be rejected");
    assert!(error.to_string().contains("servers[0] TLS requires at least one server_name"));
}

#[test]
fn deserialize_rejects_legacy_vhost_server_tls_with_policy_fields() {
    let error = ron::from_str::<Config>(
        r#"Config(
    runtime: RuntimeConfig(
        shutdown_timeout_secs: 10,
    ),
    server: ServerConfig(
        listen: "127.0.0.1:8080",
    ),
    upstreams: [
        UpstreamConfig(
            name: "backend",
            peers: [UpstreamPeerConfig(url: "http://127.0.0.1:9000")],
        ),
    ],
    locations: [
        LocationConfig(
            matcher: Prefix("/"),
            handler: Proxy(upstream: "backend"),
        ),
    ],
    servers: [
        VirtualHostConfig(
            server_names: ["api.example.com"],
            locations: [
                LocationConfig(
                    matcher: Exact("/"),
                    handler: Return(status: 200, location: "", body: Some("ok\n")),
                ),
            ],
            tls: Some(ServerTlsConfig(
                cert_path: "server.crt",
                key_path: "server.key",
                versions: Some([Tls13]),
            )),
        ),
    ],
)"#,
    )
    .expect_err("legacy vhost ServerTlsConfig with policy fields should be rejected");

    assert!(error.to_string().contains("vhost TLS policy fields are not supported"));
}

#[test]
fn validate_rejects_vhost_tls_without_any_tls_listener() {
    let mut config = base_config();
    config.server.listen = None;
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
    config.servers = vec![VirtualHostConfig {
        server_names: vec!["api.example.com".to_string()],
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
        tls: Some(VirtualHostTlsConfig {
            cert_path: "server.crt".to_string(),
            key_path: "server.key".to_string(),
            additional_certificates: None,
            ocsp_staple_path: None,
            ocsp: None,
        }),
    }];

    let error = validate(&config).expect_err("vhost tls should require a tls listener");
    assert!(error.to_string().contains("requires at least one listener with tls"));
}
