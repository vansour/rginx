use super::*;

fn listener(name: &str, listen: &str, tls: Option<ServerTlsConfig>) -> ListenerConfig {
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

fn server_tls(cert_path: &str, key_path: &str) -> ServerTlsConfig {
    ServerTlsConfig {
        cert_path: cert_path.to_string(),
        key_path: key_path.to_string(),
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
    }
}

fn managed_vhost(
    server_names: Vec<&str>,
    domains: Vec<&str>,
    cert_path: &str,
    key_path: &str,
) -> VirtualHostConfig {
    VirtualHostConfig {
        listen: Vec::new(),
        server_names: server_names.into_iter().map(str::to_string).collect(),
        upstreams: Vec::new(),
        locations: vec![test_location(
            MatcherConfig::Exact("/".to_string()),
            HandlerConfig::Return {
                status: 200,
                location: String::new(),
                body: Some("managed\n".to_string()),
            },
        )],
        tls: Some(crate::model::VirtualHostTlsConfig {
            acme: Some(crate::model::VirtualHostAcmeConfig {
                domains: domains.into_iter().map(str::to_string).collect(),
                challenge: None,
            }),
            cert_path: cert_path.to_string(),
            key_path: key_path.to_string(),
            additional_certificates: None,
            ocsp_staple_path: None,
            ocsp: None,
        }),
        http3: None,
    }
}

fn shared_tls_vhost(server_names: Vec<&str>, cert_path: &str, key_path: &str) -> VirtualHostConfig {
    VirtualHostConfig {
        listen: Vec::new(),
        server_names: server_names.into_iter().map(str::to_string).collect(),
        upstreams: Vec::new(),
        locations: vec![test_location(
            MatcherConfig::Exact("/".to_string()),
            HandlerConfig::Return {
                status: 200,
                location: String::new(),
                body: Some("shared\n".to_string()),
            },
        )],
        tls: Some(crate::model::VirtualHostTlsConfig {
            acme: None,
            cert_path: cert_path.to_string(),
            key_path: key_path.to_string(),
            additional_certificates: None,
            ocsp_staple_path: None,
            ocsp: None,
        }),
        http3: None,
    }
}

fn managed_acme_config(
    directory_url: &str,
    state_dir: &str,
    renew_before_days: Option<u64>,
    poll_interval_secs: Option<u64>,
    server_names: Vec<&str>,
    domains: Vec<&str>,
) -> Config {
    let cert_path = "managed.crt";
    let key_path = "managed.key";

    Config {
        acme: Some(crate::model::AcmeConfig {
            directory_url: directory_url.to_string(),
            contacts: vec![
                " mailto:ops@example.com ".to_string(),
                "mailto:security@example.com".to_string(),
            ],
            state_dir: state_dir.to_string(),
            renew_before_days,
            poll_interval_secs,
        }),
        cache_zones: Vec::new(),
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: vec![
            listener("http", "127.0.0.1:80", None),
            listener("https", "127.0.0.1:443", Some(server_tls(cert_path, key_path))),
        ],
        server: ServerConfig {
            listen: None,
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
        upstreams: Vec::new(),
        locations: Vec::new(),
        servers: vec![managed_vhost(server_names, domains, cert_path, key_path)],
    }
}

#[test]
fn compile_resolves_global_acme_settings_relative_to_base() {
    let base_dir = temp_base_dir("rginx-acme-global-");
    fs::write(base_dir.path().join("managed.crt"), b"placeholder")
        .expect("managed cert should be written");
    fs::write(base_dir.path().join("managed.key"), b"placeholder")
        .expect("managed key should be written");

    let config = managed_acme_config(
        " https://acme-staging-v02.api.letsencrypt.org/directory ",
        "state/acme",
        Some(21),
        Some(600),
        vec!["api.example.com"],
        vec!["api.example.com"],
    );

    let snapshot =
        compile_with_base(config, base_dir.path()).expect("managed ACME config should compile");
    let acme = snapshot.acme.expect("compiled snapshot should include ACME settings");

    assert_eq!(acme.directory_url, "https://acme-staging-v02.api.letsencrypt.org/directory");
    assert_eq!(
        acme.contacts,
        vec!["mailto:ops@example.com".to_string(), "mailto:security@example.com".to_string(),]
    );
    assert_eq!(acme.state_dir, base_dir.path().join("state/acme"));
    assert_eq!(acme.renew_before, Duration::from_secs(21 * 86_400));
    assert_eq!(acme.poll_interval, Duration::from_secs(600));
}

#[test]
fn compile_emits_managed_certificate_specs_for_acme_vhosts() {
    let base_dir = temp_base_dir("rginx-acme-managed-spec-");
    fs::write(base_dir.path().join("managed.crt"), b"placeholder")
        .expect("managed cert should be written");
    fs::write(base_dir.path().join("managed.key"), b"placeholder")
        .expect("managed key should be written");

    let config = managed_acme_config(
        "https://acme-staging-v02.api.letsencrypt.org/directory",
        "state/acme",
        None,
        None,
        vec!["API.EXAMPLE.COM", "www.example.com"],
        vec![" api.example.com ", "WWW.EXAMPLE.COM"],
    );

    let snapshot =
        compile_with_base(config, base_dir.path()).expect("managed ACME config should compile");

    let acme = snapshot.acme.as_ref().expect("compiled snapshot should include ACME settings");
    assert_eq!(acme.renew_before, Duration::from_secs(30 * 86_400));
    assert_eq!(acme.poll_interval, Duration::from_secs(3600));

    assert_eq!(snapshot.managed_certificates.len(), 1);
    let spec = &snapshot.managed_certificates[0];
    assert_eq!(spec.scope, "servers[0]");
    assert_eq!(spec.domains, vec!["api.example.com".to_string(), "www.example.com".to_string()]);
    assert_eq!(spec.cert_path, base_dir.path().join("managed.crt"));
    assert_eq!(spec.key_path, base_dir.path().join("managed.key"));
    assert_eq!(spec.challenge, rginx_core::AcmeChallengeType::Http01);
}

#[test]
fn compile_allows_missing_managed_identity_files_for_acme_issue_mode() {
    let base_dir = temp_base_dir("rginx-acme-missing-identity-");
    let config = managed_acme_config(
        "https://acme-staging-v02.api.letsencrypt.org/directory",
        "state/acme",
        None,
        None,
        vec!["api.example.com"],
        vec!["api.example.com"],
    );

    let snapshot = crate::compile::compile_with_base_and_options(
        config,
        base_dir.path(),
        crate::compile::CompileOptions { allow_missing_managed_tls_identity: true },
    )
    .expect("ACME issue mode should compile without existing managed certificate files");

    assert_eq!(snapshot.managed_certificates.len(), 1);
    let spec = &snapshot.managed_certificates[0];
    assert_eq!(spec.cert_path, base_dir.path().join("managed.crt"));
    assert_eq!(spec.key_path, base_dir.path().join("managed.key"));
}

#[test]
fn compile_allows_shared_managed_identity_files_for_acme_issue_mode() {
    let base_dir = temp_base_dir("rginx-acme-shared-identity-");
    let mut config = managed_acme_config(
        "https://acme-staging-v02.api.letsencrypt.org/directory",
        "state/acme",
        None,
        None,
        vec!["api.example.com"],
        vec!["api.example.com"],
    );
    config.servers.push(shared_tls_vhost(vec!["www.example.com"], "managed.crt", "managed.key"));

    let snapshot = crate::compile::compile_with_base_and_options(
        config,
        base_dir.path(),
        crate::compile::CompileOptions { allow_missing_managed_tls_identity: true },
    )
    .expect("shared managed identities should compile in ACME issue mode");

    assert_eq!(snapshot.managed_certificates.len(), 1);
    assert_eq!(
        snapshot.vhosts[1].tls.as_ref().expect("shared vhost should keep TLS").cert_path,
        base_dir.path().join("managed.crt")
    );
}
