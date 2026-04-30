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
        acme: None,
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
        client_ip_header: None,
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
        listen: Vec::new(),
        server_names: vec!["api.example.com".to_string()],
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
        tls: Some(VirtualHostTlsConfig {
            acme: None,
            cert_path: "server.crt".to_string(),
            key_path: "server.key".to_string(),
            additional_certificates: None,
            ocsp_staple_path: None,
            ocsp: None,
        }),
        http3: None,
    }];

    let error = validate(&config).expect_err("vhost tls should require a tls listener");
    assert!(error.to_string().contains("requires at least one listener with tls"));
}

#[test]
fn validate_accepts_vhost_listen_without_main_server_listen() {
    let mut config = base_config();
    config.server.listen = None;
    config.locations.clear();
    let mut vhost = sample_vhost(vec!["api.example.com"]);
    vhost.listen = vec!["127.0.0.1:8080".to_string()];
    config.servers = vec![vhost];

    validate(&config).expect("servers[].listen should provide the listener binding");
}

#[test]
fn validate_accepts_server_tls_defaults_with_vhost_listen() {
    let mut config = base_config();
    config.server.listen = None;
    config.server.default_certificate = Some("api.example.com".to_string());
    config.server.tls = Some(valid_server_tls());
    config.locations.clear();
    let mut vhost = sample_vhost(vec!["api.example.com"]);
    vhost.listen = vec!["127.0.0.1:8443 ssl http2".to_string()];
    vhost.tls = Some(VirtualHostTlsConfig {
        acme: None,
        cert_path: "api.crt".to_string(),
        key_path: "api.key".to_string(),
        additional_certificates: None,
        ocsp_staple_path: None,
        ocsp: None,
    });
    config.servers = vec![vhost];

    validate(&config).expect("server TLS defaults should be allowed with servers[].listen");
}

#[test]
fn validate_rejects_mixing_main_server_listen_with_vhost_listen() {
    let mut config = base_config();
    let mut vhost = sample_vhost(vec!["api.example.com"]);
    vhost.listen = vec!["127.0.0.1:8080".to_string()];
    config.servers = vec![vhost];

    let error = validate(&config).expect_err("mixed listener architecture should fail");
    assert!(error.to_string().contains("cannot be used together with servers[].listen"));
}

#[test]
fn validate_rejects_server_http3_with_vhost_listen() {
    let mut config = base_config();
    config.server.listen = None;
    config.server.tls = Some(valid_server_tls());
    config.server.http3 = Some(Http3Config::default());
    config.locations.clear();
    let mut vhost = sample_vhost(vec!["api.example.com"]);
    vhost.listen = vec!["127.0.0.1:8080".to_string()];
    config.servers = vec![vhost];

    let error = validate(&config).expect_err("server http3 should stay listener-scoped");
    assert!(error.to_string().contains("server listen, proxy_protocol, and http3"));
}

#[test]
fn validate_rejects_missing_listen_when_any_vhost_uses_listen() {
    let mut config = base_config();
    config.server.listen = None;
    config.locations.clear();
    let mut first = sample_vhost(vec!["api.example.com"]);
    first.listen = vec!["127.0.0.1:8080".to_string()];
    let second = sample_vhost(vec!["www.example.com"]);
    config.servers = vec![first, second];

    let error = validate(&config).expect_err("all vhosts should declare listen in vhost model");
    assert!(error.to_string().contains("every vhost must declare listen explicitly"));
}

#[test]
fn validate_rejects_unsupported_vhost_listen_options() {
    for option in ["default_server", "reuseport"] {
        let mut config = base_config();
        config.server.listen = None;
        config.locations.clear();
        let mut vhost = sample_vhost(vec!["api.example.com"]);
        vhost.listen = vec![format!("127.0.0.1:8080 {option}")];
        config.servers = vec![vhost];

        let error = validate(&config).expect_err("unsupported listen option should fail");
        assert!(error.to_string().contains(&format!("listen option `{option}` is not supported")));
    }
}

#[test]
fn validate_rejects_inconsistent_http3_on_shared_vhost_listen() {
    let mut config = base_config();
    config.server.listen = None;
    config.server.tls = Some(valid_server_tls());
    config.locations.clear();
    let mut first = sample_vhost(vec!["api.example.com"]);
    first.listen = vec!["127.0.0.1:8443 ssl http2 http3".to_string()];
    first.tls = Some(VirtualHostTlsConfig {
        acme: None,
        cert_path: "api.crt".to_string(),
        key_path: "api.key".to_string(),
        additional_certificates: None,
        ocsp_staple_path: None,
        ocsp: None,
    });
    first.http3 = Some(Http3Config { alt_svc_max_age_secs: Some(7200), ..Http3Config::default() });
    let mut second = sample_vhost(vec!["www.example.com"]);
    second.listen = vec!["127.0.0.1:8443 ssl http2 http3".to_string()];
    second.tls = Some(VirtualHostTlsConfig {
        acme: None,
        cert_path: "www.crt".to_string(),
        key_path: "www.key".to_string(),
        additional_certificates: None,
        ocsp_staple_path: None,
        ocsp: None,
    });
    second.http3 = Some(Http3Config { alt_svc_max_age_secs: Some(3600), ..Http3Config::default() });
    config.servers = vec![first, second];

    let error = validate(&config).expect_err("shared http3 listener settings should match");
    assert!(error.to_string().contains("must use consistent http3 settings"));
}

#[test]
fn validate_rejects_vhost_http3_when_server_tls_policy_disables_tls13() {
    let mut config = base_config();
    config.server.listen = None;
    let mut tls = valid_server_tls();
    tls.versions = Some(vec![TlsVersionConfig::Tls12]);
    config.server.tls = Some(tls);
    config.locations.clear();
    let mut vhost = sample_vhost(vec!["api.example.com"]);
    vhost.listen = vec!["127.0.0.1:8443 ssl http2 http3".to_string()];
    vhost.tls = Some(VirtualHostTlsConfig {
        acme: None,
        cert_path: "api.crt".to_string(),
        key_path: "api.key".to_string(),
        additional_certificates: None,
        ocsp_staple_path: None,
        ocsp: None,
    });
    vhost.http3 = Some(Http3Config::default());
    config.servers = vec![vhost];

    let error = validate(&config).expect_err("http3 should honor server TLS policy defaults");
    assert!(error.to_string().contains("http3 requires TLS1.3"));
}

#[test]
fn validate_rejects_vhost_ssl_listen_without_vhost_tls() {
    let mut config = base_config();
    config.server.listen = None;
    config.locations.clear();
    let mut vhost = sample_vhost(vec!["api.example.com"]);
    vhost.listen = vec!["127.0.0.1:8443 ssl http2".to_string()];
    config.servers = vec![vhost];

    let error = validate(&config).expect_err("ssl listen without vhost tls should fail");
    assert!(error.to_string().contains("servers[0] ssl listen requires tls"));
}

#[test]
fn validate_accepts_vhost_local_upstream_scope() {
    let mut config = base_config();
    config.server.listen = None;
    config.upstreams.clear();
    config.locations.clear();
    let mut vhost = sample_vhost(vec!["api.example.com"]);
    vhost.listen = vec!["127.0.0.1:8080".to_string()];
    vhost.upstreams = vec![local_upstream("backend")];
    vhost.locations[0].handler = HandlerConfig::Proxy {
        upstream: "backend".to_string(),
        preserve_host: None,
        strip_prefix: None,
        proxy_set_headers: std::collections::HashMap::new(),
    };
    config.servers = vec![vhost];

    validate(&config).expect("vhost route should see vhost-local upstream");
}

#[test]
fn validate_keeps_vhost_local_upstream_hidden_from_default_routes() {
    let mut config = base_config();
    config.upstreams.clear();
    config.servers = vec![{
        let mut vhost = sample_vhost(vec!["api.example.com"]);
        vhost.upstreams = vec![local_upstream("backend")];
        vhost
    }];

    let error = validate(&config).expect_err("default route must not see vhost-local upstream");
    assert!(error.to_string().contains("proxy upstream `backend` is not defined"));
}
