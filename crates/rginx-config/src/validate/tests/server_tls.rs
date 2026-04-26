use super::*;

#[test]
fn validate_rejects_invalid_trusted_proxy() {
    let mut config = base_config();
    config.server.trusted_proxies = vec!["bad-proxy".to_string()];

    let error = validate(&config).expect_err("invalid trusted proxy should be rejected");
    assert!(error.to_string().contains("server trusted_proxies entry `bad-proxy`"));
}

#[test]
fn validate_rejects_empty_server_tls_cert_path() {
    let mut config = base_config();
    config.server.tls = Some(ServerTlsConfig {
        cert_path: " ".to_string(),
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
    });

    let error = validate(&config).expect_err("empty cert path should be rejected");
    assert!(error.to_string().contains("server TLS certificate path must not be empty"));
}

#[test]
fn validate_rejects_empty_default_certificate_name() {
    let mut config = base_config();
    config.server.default_certificate = Some("   ".to_string());

    let error = validate(&config).expect_err("blank default_certificate should be rejected");
    assert!(error.to_string().contains("server default_certificate must not be empty"));
}

#[test]
fn validate_rejects_empty_server_tls_versions_list() {
    let mut config = base_config();
    config.server.tls = Some(ServerTlsConfig {
        cert_path: "server.crt".to_string(),
        key_path: "server.key".to_string(),
        additional_certificates: None,
        versions: Some(Vec::new()),
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
    });

    let error = validate(&config).expect_err("empty TLS versions should be rejected");
    assert!(error.to_string().contains("server TLS versions must not be empty"));
}

#[test]
fn validate_rejects_empty_server_tls_cipher_suites_list() {
    let mut config = base_config();
    config.server.tls = Some(ServerTlsConfig {
        cert_path: "server.crt".to_string(),
        key_path: "server.key".to_string(),
        additional_certificates: None,
        versions: None,
        cipher_suites: Some(Vec::new()),
        key_exchange_groups: None,
        alpn_protocols: None,
        ocsp_staple_path: None,
        ocsp: None,
        session_resumption: None,
        session_tickets: None,
        session_cache_size: None,
        session_ticket_count: None,
        client_auth: None,
    });

    let error = validate(&config).expect_err("empty TLS cipher_suites should be rejected");
    assert!(error.to_string().contains("TLS cipher_suites must not be empty"));
}

#[test]
fn validate_rejects_incompatible_cipher_suites_and_versions() {
    let mut config = base_config();
    config.server.tls = Some(ServerTlsConfig {
        cert_path: "server.crt".to_string(),
        key_path: "server.key".to_string(),
        additional_certificates: None,
        versions: Some(vec![crate::model::TlsVersionConfig::Tls12]),
        cipher_suites: Some(vec![TlsCipherSuiteConfig::Tls13Aes128GcmSha256]),
        key_exchange_groups: None,
        alpn_protocols: None,
        ocsp_staple_path: None,
        ocsp: None,
        session_resumption: None,
        session_tickets: None,
        session_cache_size: None,
        session_ticket_count: None,
        client_auth: None,
    });

    let error = validate(&config).expect_err("cipher/version mismatch should be rejected");
    assert!(error.to_string().contains("do not support any configured TLS versions"));
}

#[test]
fn validate_rejects_session_tickets_when_resumption_is_disabled() {
    let mut config = base_config();
    config.server.tls = Some(ServerTlsConfig {
        cert_path: "server.crt".to_string(),
        key_path: "server.key".to_string(),
        additional_certificates: None,
        versions: None,
        cipher_suites: None,
        key_exchange_groups: Some(vec![TlsKeyExchangeGroupConfig::X25519]),
        alpn_protocols: None,
        ocsp_staple_path: None,
        ocsp: None,
        session_resumption: Some(false),
        session_tickets: Some(true),
        session_cache_size: None,
        session_ticket_count: None,
        client_auth: None,
    });

    let error = validate(&config).expect_err("tickets require resumption");
    assert!(error.to_string().contains("session_tickets requires session_resumption"));
}

#[test]
fn validate_rejects_session_cache_size_when_resumption_is_disabled() {
    let mut config = base_config();
    config.server.tls = Some(ServerTlsConfig {
        cert_path: "server.crt".to_string(),
        key_path: "server.key".to_string(),
        additional_certificates: None,
        versions: None,
        cipher_suites: None,
        key_exchange_groups: None,
        alpn_protocols: None,
        ocsp_staple_path: None,
        ocsp: None,
        session_resumption: Some(false),
        session_tickets: None,
        session_cache_size: Some(128),
        session_ticket_count: None,
        client_auth: None,
    });

    let error = validate(&config).expect_err("cache size requires resumption");
    assert!(error.to_string().contains("session_cache_size cannot be set"));
}

#[test]
fn validate_rejects_session_ticket_count_when_tickets_are_disabled() {
    let mut config = base_config();
    config.server.tls = Some(ServerTlsConfig {
        cert_path: "server.crt".to_string(),
        key_path: "server.key".to_string(),
        additional_certificates: None,
        versions: None,
        cipher_suites: None,
        key_exchange_groups: None,
        alpn_protocols: None,
        ocsp_staple_path: None,
        ocsp: None,
        session_resumption: Some(true),
        session_tickets: Some(false),
        session_cache_size: None,
        session_ticket_count: Some(2),
        client_auth: None,
    });

    let error = validate(&config).expect_err("ticket count cannot override disabled tickets");
    assert!(
        error
            .to_string()
            .contains("session_ticket_count cannot be set when session_tickets is disabled")
    );
}

#[test]
fn validate_rejects_zero_session_ticket_count() {
    let mut config = base_config();
    config.server.tls = Some(ServerTlsConfig {
        cert_path: "server.crt".to_string(),
        key_path: "server.key".to_string(),
        additional_certificates: None,
        versions: None,
        cipher_suites: None,
        key_exchange_groups: None,
        alpn_protocols: None,
        ocsp_staple_path: None,
        ocsp: None,
        session_resumption: Some(true),
        session_tickets: Some(true),
        session_cache_size: None,
        session_ticket_count: Some(0),
        client_auth: None,
    });

    let error = validate(&config).expect_err("zero ticket count should be rejected");
    assert!(error.to_string().contains("session_ticket_count must be greater than 0"));
}
