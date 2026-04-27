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
        listen: Vec::new(),
        server_names: vec!["api.example.com".to_string()],
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
        tls: Some(VirtualHostTlsConfig {
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
fn validate_rejects_mixing_main_server_listen_with_vhost_listen() {
    let mut config = base_config();
    let mut vhost = sample_vhost(vec!["api.example.com"]);
    vhost.listen = vec!["127.0.0.1:8080".to_string()];
    config.servers = vec![vhost];

    let error = validate(&config).expect_err("mixed listener architecture should fail");
    assert!(error.to_string().contains("cannot be used together with servers[].listen"));
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

fn local_upstream(name: &str) -> UpstreamConfig {
    UpstreamConfig {
        name: name.to_string(),
        peers: vec![UpstreamPeerConfig {
            url: "http://127.0.0.1:9000".to_string(),
            weight: 1,
            backup: false,
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
        health_check_grpc_service: None,
        health_check_interval_secs: None,
        health_check_timeout_secs: None,
        healthy_successes_required: None,
    }
}
