use super::*;

fn explicit_listener(name: &str, listen: &str, tls: Option<ServerTlsConfig>) -> ListenerConfig {
    ListenerConfig {
        name: name.to_string(),
        server_header: None,
        proxy_protocol: None,
        default_certificate: None,
        listen: listen.to_string(),
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
        tls,
        http3: None,
    }
}

fn valid_global_acme() -> crate::model::AcmeConfig {
    crate::model::AcmeConfig {
        directory_url: "https://acme-staging-v02.api.letsencrypt.org/directory".to_string(),
        contacts: vec!["mailto:ops@example.com".to_string()],
        state_dir: "var/acme".to_string(),
        renew_before_days: None,
        poll_interval_secs: None,
    }
}

fn managed_vhost(server_names: Vec<&str>, domains: Vec<&str>) -> VirtualHostConfig {
    let mut vhost = sample_vhost(server_names);
    vhost.tls = Some(VirtualHostTlsConfig {
        acme: Some(crate::model::VirtualHostAcmeConfig {
            domains: domains.into_iter().map(str::to_string).collect(),
            challenge: None,
        }),
        cert_path: "managed.crt".to_string(),
        key_path: "managed.key".to_string(),
        additional_certificates: None,
        ocsp_staple_path: None,
        ocsp: None,
    });
    vhost
}

fn enable_acme_listeners(config: &mut Config) {
    config.server.listen = None;
    config.listeners = vec![
        explicit_listener("http", "127.0.0.1:80", None),
        explicit_listener("https", "127.0.0.1:443", Some(valid_server_tls())),
    ];
}

#[test]
fn deserialize_rejects_legacy_vhost_server_tls_with_acme_field() {
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
                cert_path: "managed.crt",
                key_path: "managed.key",
                acme: Some(VirtualHostAcmeConfig(
                    domains: ["api.example.com"],
                )),
            )),
        ),
    ],
)"#,
    )
    .expect_err("legacy vhost ServerTlsConfig with ACME should be rejected");

    assert!(error.to_string().contains("vhost TLS policy fields are not supported"));
}

#[test]
fn validate_rejects_empty_global_acme_directory_url() {
    let mut config = base_config();
    config.acme =
        Some(crate::model::AcmeConfig { directory_url: "   ".to_string(), ..valid_global_acme() });

    let error = validate(&config).expect_err("empty ACME directory_url should be rejected");
    assert!(error.to_string().contains("acme.directory_url must not be empty"));
}

#[test]
fn validate_rejects_empty_global_acme_state_dir() {
    let mut config = base_config();
    config.acme =
        Some(crate::model::AcmeConfig { state_dir: " ".to_string(), ..valid_global_acme() });

    let error = validate(&config).expect_err("empty ACME state_dir should be rejected");
    assert!(error.to_string().contains("acme.state_dir must not be empty"));
}

#[test]
fn validate_rejects_zero_global_acme_renew_before_days() {
    let mut config = base_config();
    config.acme =
        Some(crate::model::AcmeConfig { renew_before_days: Some(0), ..valid_global_acme() });

    let error = validate(&config).expect_err("zero ACME renew_before_days should be rejected");
    assert!(error.to_string().contains("acme.renew_before_days must be greater than 0"));
}

#[test]
fn validate_rejects_zero_global_acme_poll_interval_secs() {
    let mut config = base_config();
    config.acme =
        Some(crate::model::AcmeConfig { poll_interval_secs: Some(0), ..valid_global_acme() });

    let error = validate(&config).expect_err("zero ACME poll_interval_secs should be rejected");
    assert!(error.to_string().contains("acme.poll_interval_secs must be greater than 0"));
}

#[test]
fn validate_accepts_global_acme_without_contacts() {
    let mut config = base_config();
    config.acme = Some(crate::model::AcmeConfig { contacts: Vec::new(), ..valid_global_acme() });

    validate(&config).expect("phase 1 should allow ACME accounts without explicit contacts");
}

#[test]
fn validate_rejects_managed_vhost_without_global_acme() {
    let mut config = base_config();
    enable_acme_listeners(&mut config);
    config.servers = vec![managed_vhost(vec!["api.example.com"], vec!["api.example.com"])];

    let error = validate(&config).expect_err("managed vhost should require top-level ACME");
    assert!(
        error.to_string().contains("servers[0] TLS ACME requires top-level acme configuration")
    );
}

#[test]
fn validate_rejects_managed_vhost_with_empty_domains() {
    let mut config = base_config();
    config.acme = Some(valid_global_acme());
    enable_acme_listeners(&mut config);
    config.servers = vec![managed_vhost(vec!["api.example.com"], Vec::new())];

    let error = validate(&config).expect_err("managed vhost without domains should be rejected");
    assert!(error.to_string().contains("servers[0] TLS ACME domains must not be empty"));
}

#[test]
fn validate_rejects_managed_vhost_with_wildcard_domain() {
    let mut config = base_config();
    config.acme = Some(valid_global_acme());
    enable_acme_listeners(&mut config);
    config.servers = vec![managed_vhost(vec!["api.example.com"], vec!["*.example.com"])];

    let error = validate(&config).expect_err("wildcard ACME domains should be rejected");
    assert!(error.to_string().contains("servers[0] TLS ACME domains[0] wildcard `*.example.com`"));
}

#[test]
fn validate_rejects_managed_vhost_with_wildcard_server_name() {
    let mut config = base_config();
    config.acme = Some(valid_global_acme());
    enable_acme_listeners(&mut config);
    config.servers = vec![managed_vhost(vec!["*.example.com"], vec!["*.example.com"])];

    let error = validate(&config).expect_err("wildcard server_names should be rejected for ACME");
    assert!(error.to_string().contains("servers[0] server_names[0] wildcard `*.example.com`"));
}

#[test]
fn validate_rejects_managed_vhost_with_domain_server_name_mismatch() {
    let mut config = base_config();
    config.acme = Some(valid_global_acme());
    enable_acme_listeners(&mut config);
    config.servers = vec![managed_vhost(vec!["api.example.com"], vec!["www.example.com"])];

    let error =
        validate(&config).expect_err("managed vhost domains should match server_names exactly");
    assert!(
        error
            .to_string()
            .contains("servers[0] TLS ACME domains must match server_names exactly in phase 1")
    );
}

#[test]
fn validate_rejects_managed_vhost_with_additional_certificates() {
    let mut config = base_config();
    config.acme = Some(valid_global_acme());
    enable_acme_listeners(&mut config);
    let mut vhost = managed_vhost(vec!["api.example.com"], vec!["api.example.com"]);
    vhost.tls.as_mut().expect("managed vhost should define tls").additional_certificates =
        Some(vec![crate::model::ServerCertificateBundleConfig {
            cert_path: "backup.crt".to_string(),
            key_path: "backup.key".to_string(),
            ocsp_staple_path: None,
            ocsp: None,
        }]);
    config.servers = vec![vhost];

    let error = validate(&config).expect_err("managed vhost should reject additional certificates");
    assert!(
        error
            .to_string()
            .contains("servers[0] TLS ACME does not support additional_certificates in phase 1")
    );
}

#[test]
fn validate_rejects_managed_vhost_without_plain_http_80_listener() {
    let mut config = base_config();
    config.acme = Some(valid_global_acme());
    config.server.listen = None;
    config.listeners = vec![explicit_listener("https", "127.0.0.1:443", Some(valid_server_tls()))];
    config.servers = vec![managed_vhost(vec!["api.example.com"], vec!["api.example.com"])];

    let error = validate(&config).expect_err("HTTP-01 should require a plain HTTP :80 listener");
    assert!(
        error
            .to_string()
            .contains("ACME HTTP-01 requires at least one plain HTTP listener bound to port 80")
    );
}

#[test]
fn validate_rejects_managed_vhost_with_legacy_server_listen_only() {
    let mut config = base_config();
    config.acme = Some(valid_global_acme());
    config.server.listen = Some("127.0.0.1:80".to_string());
    config.servers = vec![managed_vhost(vec!["api.example.com"], vec!["api.example.com"])];

    let error = validate(&config).expect_err(
        "legacy server.listen should not count as plain HTTP once vhost TLS is managed",
    );
    assert!(
        error
            .to_string()
            .contains("ACME HTTP-01 requires at least one plain HTTP listener bound to port 80")
    );
}

#[test]
fn validate_rejects_managed_vhost_with_http3_enabled_explicit_listener() {
    let mut config = base_config();
    config.acme = Some(valid_global_acme());
    enable_acme_listeners(&mut config);
    config.listeners[1].http3 = Some(crate::model::Http3Config::default());
    config.servers = vec![managed_vhost(vec!["api.example.com"], vec!["api.example.com"])];

    let error = validate(&config)
        .expect_err("managed ACME should reject explicit listeners with http3 enabled");
    assert!(error.to_string().contains("listeners[1] enables http3"));
}

#[test]
fn validate_rejects_managed_vhost_with_http3_enabled_vhost_binding() {
    let mut config = base_config();
    config.acme = Some(valid_global_acme());
    config.server.listen = None;

    let mut vhost = managed_vhost(vec!["api.example.com"], vec!["api.example.com"]);
    vhost.listen = vec!["127.0.0.1:80".to_string(), "127.0.0.1:443 ssl http2 http3".to_string()];
    vhost.http3 = Some(crate::model::Http3Config::default());
    config.servers = vec![vhost];

    let error = validate(&config)
        .expect_err("managed ACME should reject vhost listeners with http3 enabled");
    assert!(
        error.to_string().contains("servers[0].listen[1] ACME-managed TLS does not support http3")
    );
}

#[test]
fn validate_accepts_managed_vhost_when_unmanaged_http3_uses_separate_binding() {
    let mut config = base_config();
    config.acme = Some(valid_global_acme());
    config.server.listen = None;

    let mut managed = managed_vhost(vec!["api.example.com"], vec!["api.example.com"]);
    managed.listen = vec!["127.0.0.1:80".to_string(), "127.0.0.1:443 ssl http2".to_string()];

    let mut static_http3 = sample_vhost(vec!["static.example.com"]);
    static_http3.listen =
        vec!["127.0.0.1:8443 ssl http2 http3".to_string(), "127.0.0.1:8080".to_string()];
    static_http3.http3 = Some(crate::model::Http3Config::default());
    static_http3.tls = Some(VirtualHostTlsConfig {
        acme: None,
        cert_path: "static.crt".to_string(),
        key_path: "static.key".to_string(),
        additional_certificates: None,
        ocsp_staple_path: None,
        ocsp: None,
    });

    config.servers = vec![managed, static_http3];

    validate(&config)
        .expect("managed ACME should allow separate unmanaged http3 bindings in vhost mode");
}

#[test]
fn validate_accepts_phase1_managed_vhost_configuration() {
    let mut config = base_config();
    config.acme = Some(valid_global_acme());
    enable_acme_listeners(&mut config);
    config.servers = vec![managed_vhost(vec!["api.example.com"], vec!["api.example.com"])];

    validate(&config).expect("phase 1 managed vhost ACME config should validate");
}
