use super::*;

#[test]
fn validate_rejects_empty_server_access_log_format() {
    let mut config = base_config();
    config.server.access_log_format = Some("   ".to_string());

    let error = validate(&config).expect_err("empty access log format should be rejected");
    assert!(error.to_string().contains("server access_log_format must not be empty"));
}

#[test]
fn validate_rejects_empty_server_header() {
    let mut config = base_config();
    config.server.server_header = Some("   ".to_string());

    let error = validate(&config).expect_err("empty server header should be rejected");
    assert!(error.to_string().contains("server server_header must not be empty"));
}

#[test]
fn validate_rejects_http3_without_tls_on_same_listener() {
    let mut config = base_config();
    config.server.http3 = Some(Http3Config {
        listen: None,
        advertise_alt_svc: Some(true),
        alt_svc_max_age_secs: Some(3600),
        max_concurrent_streams: None,
        stream_buffer_size_bytes: None,
        active_connection_id_limit: None,
        retry: None,
        host_key_path: None,
        gso: None,
        early_data: None,
    });

    let error = validate(&config).expect_err("http3 should require tls on the same listener");
    assert!(error.to_string().contains("http3 requires tls to be configured on the same listener"));
}

#[test]
fn validate_rejects_zero_http3_max_concurrent_streams() {
    let mut config = base_config();
    config.server.tls = Some(valid_server_tls());
    config.server.http3 = Some(Http3Config {
        listen: None,
        advertise_alt_svc: None,
        alt_svc_max_age_secs: None,
        max_concurrent_streams: Some(0),
        stream_buffer_size_bytes: None,
        active_connection_id_limit: None,
        retry: None,
        host_key_path: None,
        gso: None,
        early_data: None,
    });

    let error = validate(&config).expect_err("zero max_concurrent_streams should be rejected");
    assert!(error.to_string().contains("http3 max_concurrent_streams must be greater than 0"));
}

#[test]
fn validate_rejects_zero_http3_stream_buffer_size() {
    let mut config = base_config();
    config.server.tls = Some(valid_server_tls());
    config.server.http3 = Some(Http3Config {
        listen: None,
        advertise_alt_svc: None,
        alt_svc_max_age_secs: None,
        max_concurrent_streams: None,
        stream_buffer_size_bytes: Some(0),
        active_connection_id_limit: None,
        retry: None,
        host_key_path: None,
        gso: None,
        early_data: None,
    });

    let error = validate(&config).expect_err("zero stream_buffer_size_bytes should be rejected");
    assert!(error.to_string().contains("http3 stream_buffer_size_bytes must be greater than 0"));
}

#[test]
fn validate_rejects_http3_active_connection_id_limit_below_two() {
    let mut config = base_config();
    config.server.tls = Some(valid_server_tls());
    config.server.http3 = Some(Http3Config {
        listen: None,
        advertise_alt_svc: None,
        alt_svc_max_age_secs: None,
        max_concurrent_streams: None,
        stream_buffer_size_bytes: None,
        active_connection_id_limit: Some(1),
        retry: None,
        host_key_path: None,
        gso: None,
        early_data: None,
    });

    let error = validate(&config).expect_err("active_connection_id_limit below two should fail");
    assert!(
        error
            .to_string()
            .contains("http3 active_connection_id_limit must be greater than or equal to 2")
    );
}

#[test]
fn validate_rejects_unsupported_http3_active_connection_id_limit() {
    let mut config = base_config();
    config.server.tls = Some(valid_server_tls());
    config.server.http3 = Some(Http3Config {
        listen: None,
        advertise_alt_svc: None,
        alt_svc_max_age_secs: None,
        max_concurrent_streams: None,
        stream_buffer_size_bytes: None,
        active_connection_id_limit: Some(4),
        retry: None,
        host_key_path: None,
        gso: None,
        early_data: None,
    });

    let error = validate(&config).expect_err("unsupported active_connection_id_limit should fail");
    assert!(
        error
            .to_string()
            .contains("http3 active_connection_id_limit currently supports only 2 or 5")
    );
}

#[test]
fn validate_rejects_blank_http3_host_key_path() {
    let mut config = base_config();
    config.server.tls = Some(valid_server_tls());
    config.server.http3 = Some(Http3Config {
        listen: None,
        advertise_alt_svc: None,
        alt_svc_max_age_secs: None,
        max_concurrent_streams: None,
        stream_buffer_size_bytes: None,
        active_connection_id_limit: None,
        retry: None,
        host_key_path: Some("  ".to_string()),
        gso: None,
        early_data: None,
    });

    let error = validate(&config).expect_err("blank host_key_path should be rejected");
    assert!(error.to_string().contains("http3 host_key_path must not be empty"));
}

#[test]
fn validate_rejects_http3_early_data_when_session_resumption_is_disabled() {
    let mut config = base_config();
    let mut tls = valid_server_tls();
    tls.session_resumption = Some(false);
    config.server.tls = Some(tls);
    config.server.http3 = Some(Http3Config {
        listen: None,
        advertise_alt_svc: None,
        alt_svc_max_age_secs: None,
        max_concurrent_streams: None,
        stream_buffer_size_bytes: None,
        active_connection_id_limit: None,
        retry: None,
        host_key_path: None,
        gso: None,
        early_data: Some(true),
    });

    let error = validate(&config).expect_err("http3 early_data should require session resumption");
    assert!(error.to_string().contains("http3 early_data requires tls session_resumption"));
}

#[test]
fn validate_rejects_http3_early_data_when_session_cache_is_disabled() {
    let mut config = base_config();
    let mut tls = valid_server_tls();
    tls.session_cache_size = Some(0);
    config.server.tls = Some(tls);
    config.server.http3 = Some(Http3Config {
        listen: None,
        advertise_alt_svc: None,
        alt_svc_max_age_secs: None,
        max_concurrent_streams: None,
        stream_buffer_size_bytes: None,
        active_connection_id_limit: None,
        retry: None,
        host_key_path: None,
        gso: None,
        early_data: Some(true),
    });

    let error = validate(&config).expect_err("http3 early_data should require session cache");
    assert!(error.to_string().contains("http3 early_data requires tls session_cache_size"));
}

#[test]
fn validate_rejects_http3_early_data_when_session_tickets_are_enabled() {
    let mut config = base_config();
    let mut tls = valid_server_tls();
    tls.session_tickets = Some(true);
    config.server.tls = Some(tls);
    config.server.http3 = Some(Http3Config {
        listen: None,
        advertise_alt_svc: None,
        alt_svc_max_age_secs: None,
        max_concurrent_streams: None,
        stream_buffer_size_bytes: None,
        active_connection_id_limit: None,
        retry: None,
        host_key_path: None,
        gso: None,
        early_data: Some(true),
    });

    let error = validate(&config).expect_err("http3 early_data should require stateful resumption");
    assert!(error.to_string().contains("http3 early_data requires tls session_tickets"));
}

#[test]
fn validate_rejects_http3_retry_without_host_key_path() {
    let mut config = base_config();
    config.server.tls = Some(valid_server_tls());
    config.server.http3 = Some(Http3Config {
        listen: None,
        advertise_alt_svc: None,
        alt_svc_max_age_secs: None,
        max_concurrent_streams: None,
        stream_buffer_size_bytes: None,
        active_connection_id_limit: Some(5),
        retry: Some(true),
        host_key_path: None,
        gso: None,
        early_data: None,
    });

    let error = validate(&config).expect_err("retry without host_key_path should fail");
    assert!(error.to_string().contains("http3 retry requires host_key_path"));
}

#[test]
fn validate_allows_http3_with_downstream_client_auth() {
    let mut config = base_config();
    config.server.server_names = vec!["localhost".to_string()];
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
        session_resumption: None,
        session_tickets: None,
        session_cache_size: None,
        session_ticket_count: None,
        client_auth: Some(crate::model::ServerClientAuthConfig {
            mode: crate::model::ServerClientAuthModeConfig::Required,
            ca_cert_path: "client-ca.pem".to_string(),
            verify_depth: None,
            crl_path: None,
        }),
    });
    config.server.http3 = Some(Http3Config {
        listen: None,
        advertise_alt_svc: Some(true),
        alt_svc_max_age_secs: Some(3600),
        max_concurrent_streams: None,
        stream_buffer_size_bytes: None,
        active_connection_id_limit: Some(2),
        retry: None,
        host_key_path: None,
        gso: None,
        early_data: None,
    });

    validate(&config).expect("http3 should allow downstream client_auth");
}
