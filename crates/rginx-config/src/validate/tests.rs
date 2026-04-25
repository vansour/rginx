use crate::model::{
    Config, HandlerConfig, Http3Config, ListenerConfig, LocationConfig, MatcherConfig,
    RouteBufferingPolicyConfig, RouteCompressionPolicyConfig, RuntimeConfig, ServerConfig,
    ServerTlsConfig, TlsCipherSuiteConfig, TlsKeyExchangeGroupConfig, UpstreamConfig,
    UpstreamLoadBalanceConfig, UpstreamPeerConfig, UpstreamProtocolConfig, VirtualHostConfig,
    VirtualHostTlsConfig,
};

use super::{DEFAULT_GRPC_HEALTH_CHECK_PATH, validate};

#[test]
fn validate_rejects_zero_max_replayable_body_size() {
    let mut config = base_config();
    config.upstreams[0].max_replayable_request_body_bytes = Some(0);

    let error = validate(&config).expect_err("zero body size should be rejected");
    assert!(error.to_string().contains("max_replayable_request_body_bytes must be greater than 0"));
}

#[test]
fn validate_rejects_zero_unhealthy_after_failures() {
    let mut config = base_config();
    config.upstreams[0].unhealthy_after_failures = Some(0);

    let error = validate(&config).expect_err("zero failure threshold should be rejected");
    assert!(error.to_string().contains("unhealthy_after_failures must be greater than 0"));
}

#[test]
fn validate_rejects_zero_unhealthy_cooldown() {
    let mut config = base_config();
    config.upstreams[0].unhealthy_cooldown_secs = Some(0);

    let error = validate(&config).expect_err("zero cooldown should be rejected");
    assert!(error.to_string().contains("unhealthy_cooldown_secs must be greater than 0"));
}

#[test]
fn validate_rejects_zero_connect_timeout() {
    let mut config = base_config();
    config.upstreams[0].connect_timeout_secs = Some(0);

    let error = validate(&config).expect_err("zero connect timeout should be rejected");
    assert!(error.to_string().contains("connect_timeout_secs must be greater than 0"));
}

#[test]
fn validate_rejects_zero_write_timeout() {
    let mut config = base_config();
    config.upstreams[0].write_timeout_secs = Some(0);

    let error = validate(&config).expect_err("zero write timeout should be rejected");
    assert!(error.to_string().contains("write_timeout_secs must be greater than 0"));
}

#[test]
fn validate_rejects_zero_idle_timeout() {
    let mut config = base_config();
    config.upstreams[0].idle_timeout_secs = Some(0);

    let error = validate(&config).expect_err("zero idle timeout should be rejected");
    assert!(error.to_string().contains("idle_timeout_secs must be greater than 0"));
}

#[test]
fn validate_allows_disabling_pool_idle_timeout() {
    let mut config = base_config();
    config.upstreams[0].pool_idle_timeout_secs = Some(0);

    validate(&config).expect("pool_idle_timeout_secs: Some(0) should be accepted");
}

#[test]
fn validate_rejects_zero_tcp_keepalive_timeout() {
    let mut config = base_config();
    config.upstreams[0].tcp_keepalive_secs = Some(0);

    let error = validate(&config).expect_err("zero tcp keepalive should be rejected");
    assert!(error.to_string().contains("tcp_keepalive_secs must be greater than 0"));
}

#[test]
fn validate_rejects_http2_keepalive_tuning_without_interval() {
    let mut config = base_config();
    config.upstreams[0].http2_keep_alive_timeout_secs = Some(5);

    let error = validate(&config).expect_err("http2 keepalive tuning should require an interval");
    assert!(error.to_string().contains(
        "http2_keep_alive_timeout_secs and http2_keep_alive_while_idle require http2_keep_alive_interval_secs to be set"
    ));
}

#[test]
fn validate_rejects_zero_http2_keepalive_interval() {
    let mut config = base_config();
    config.upstreams[0].http2_keep_alive_interval_secs = Some(0);

    let error = validate(&config).expect_err("zero http2 keepalive interval should be rejected");
    assert!(error.to_string().contains("http2_keep_alive_interval_secs must be greater than 0"));
}

#[test]
fn validate_rejects_invalid_route_allow_cidr() {
    let mut config = base_config();
    config.locations[0].allow_cidrs = vec!["not-a-cidr".to_string()];

    let error = validate(&config).expect_err("invalid CIDR should be rejected");
    assert!(error.to_string().contains("allow_cidrs entry `not-a-cidr` is invalid"));
}

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

#[test]
fn validate_rejects_zero_route_requests_per_sec() {
    let mut config = base_config();
    config.locations[0].requests_per_sec = Some(0);

    let error = validate(&config).expect_err("zero requests_per_sec should be rejected");
    assert!(error.to_string().contains("requests_per_sec must be greater than 0"));
}

#[test]
fn validate_rejects_burst_without_rate_limit() {
    let mut config = base_config();
    config.locations[0].burst = Some(2);

    let error = validate(&config).expect_err("burst without rate limit should be rejected");
    assert!(error.to_string().contains("burst requires requests_per_sec to be set"));
}

#[test]
fn validate_rejects_zero_route_compression_min_bytes() {
    let mut config = base_config();
    config.locations[0].compression_min_bytes = Some(0);

    let error = validate(&config).expect_err("zero compression_min_bytes should be rejected");
    assert!(error.to_string().contains("compression_min_bytes must be greater than 0"));
}

#[test]
fn validate_rejects_zero_route_streaming_response_idle_timeout() {
    let mut config = base_config();
    config.locations[0].streaming_response_idle_timeout_secs = Some(0);

    let error =
        validate(&config).expect_err("zero streaming response idle timeout should be rejected");
    assert!(
        error.to_string().contains("streaming_response_idle_timeout_secs must be greater than 0")
    );
}

#[test]
fn validate_rejects_force_compression_with_response_buffering_off() {
    let mut config = base_config();
    config.locations[0].response_buffering = Some(RouteBufferingPolicyConfig::Off);
    config.locations[0].compression = Some(RouteCompressionPolicyConfig::Force);

    let error = validate(&config).expect_err("force compression should require buffering");
    assert!(
        error
            .to_string()
            .contains("compression=Force requires response_buffering to remain Auto or On")
    );
}

#[test]
fn validate_rejects_empty_route_compression_content_types() {
    let mut config = base_config();
    config.locations[0].compression_content_types = Some(Vec::new());

    let error = validate(&config).expect_err("empty compression_content_types should be rejected");
    assert!(error.to_string().contains("compression_content_types must not be empty"));
}

#[test]
fn validate_rejects_blank_route_compression_content_type_entry() {
    let mut config = base_config();
    config.locations[0].compression_content_types = Some(vec!["   ".to_string()]);

    let error =
        validate(&config).expect_err("blank compression_content_types entry should be rejected");
    assert!(error.to_string().contains("compression_content_types entries must not be empty"));
}

#[test]
fn validate_rejects_zero_runtime_worker_threads() {
    let mut config = base_config();
    config.runtime.worker_threads = Some(0);

    let error = validate(&config).expect_err("zero runtime worker threads should be rejected");
    assert!(error.to_string().contains("runtime.worker_threads must be greater than 0"));
}

#[test]
fn validate_rejects_zero_runtime_accept_workers() {
    let mut config = base_config();
    config.runtime.accept_workers = Some(0);

    let error = validate(&config).expect_err("zero runtime accept workers should be rejected");
    assert!(error.to_string().contains("runtime.accept_workers must be greater than 0"));
}

#[test]
fn validate_rejects_zero_server_max_connections() {
    let mut config = base_config();
    config.server.max_connections = Some(0);

    let error = validate(&config).expect_err("zero max connections should be rejected");
    assert!(error.to_string().contains("server max_connections must be greater than 0"));
}

#[test]
fn validate_rejects_zero_server_header_read_timeout() {
    let mut config = base_config();
    config.server.header_read_timeout_secs = Some(0);

    let error = validate(&config).expect_err("zero header timeout should be rejected");
    assert!(error.to_string().contains("server header_read_timeout_secs must be greater than 0"));
}

#[test]
fn validate_rejects_zero_server_request_body_read_timeout() {
    let mut config = base_config();
    config.server.request_body_read_timeout_secs = Some(0);

    let error = validate(&config).expect_err("zero request body read timeout should be rejected");
    assert!(
        error.to_string().contains("server request_body_read_timeout_secs must be greater than 0")
    );
}

#[test]
fn validate_rejects_zero_server_response_write_timeout() {
    let mut config = base_config();
    config.server.response_write_timeout_secs = Some(0);

    let error = validate(&config).expect_err("zero response write timeout should be rejected");
    assert!(
        error.to_string().contains("server response_write_timeout_secs must be greater than 0")
    );
}

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

#[test]
fn validate_rejects_zero_server_max_headers() {
    let mut config = base_config();
    config.server.max_headers = Some(0);

    let error = validate(&config).expect_err("zero max headers should be rejected");
    assert!(error.to_string().contains("server max_headers must be greater than 0"));
}

#[test]
fn validate_rejects_zero_server_max_request_body_bytes() {
    let mut config = base_config();
    config.server.max_request_body_bytes = Some(0);

    let error = validate(&config).expect_err("zero max request body should be rejected");
    assert!(error.to_string().contains("server max_request_body_bytes must be greater than 0"));
}

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
fn validate_rejects_empty_grpc_service() {
    let mut config = base_config();
    config.locations[0].grpc_service = Some("   ".to_string());

    let error = validate(&config).expect_err("empty grpc_service should be rejected");
    assert!(error.to_string().contains("grpc_service must not be empty"));
}

#[test]
fn validate_allows_duplicate_exact_routes_when_grpc_constraints_differ() {
    let mut config = base_config();
    config.locations[0].matcher = MatcherConfig::Exact("/grpc.health.v1.Health/Check".to_string());
    config.locations.push(LocationConfig {
        matcher: MatcherConfig::Exact("/grpc.health.v1.Health/Check".to_string()),
        handler: HandlerConfig::Proxy {
            upstream: "backend".to_string(),
            preserve_host: None,
            strip_prefix: None,
            proxy_set_headers: std::collections::HashMap::new(),
        },
        grpc_service: Some("grpc.health.v1.Health".to_string()),
        grpc_method: Some("Check".to_string()),
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
    });

    validate(&config).expect("different gRPC route constraints should be allowed");
}

#[test]
fn validate_rejects_duplicate_exact_routes_when_grpc_constraints_match() {
    let mut config = base_config();
    config.locations[0].matcher = MatcherConfig::Exact("/grpc.health.v1.Health/Check".to_string());
    config.locations[0].grpc_service = Some("grpc.health.v1.Health".to_string());
    config.locations[0].grpc_method = Some("Check".to_string());
    config.locations.push(config.locations[0].clone());

    let error =
        validate(&config).expect_err("duplicate exact route with same gRPC match should fail");
    assert!(error.to_string().contains(
        "duplicate exact route `/grpc.health.v1.Health/Check` with the same gRPC route constraints"
    ));
}

#[test]
fn validate_rejects_active_health_tuning_without_path() {
    let mut config = base_config();
    config.upstreams[0].health_check_timeout_secs = Some(1);

    let error = validate(&config).expect_err("active health tuning should require a path");
    assert!(error.to_string().contains("active health-check tuning requires health_check_path"));
}

#[test]
fn validate_allows_grpc_health_check_with_default_path() {
    let mut config = base_config();
    config.upstreams[0].peers[0].url = "https://example.com".to_string();
    config.upstreams[0].health_check_grpc_service = Some("grpc.health.v1.Health".to_string());

    validate(&config).expect("gRPC health-check config should validate");
}

#[test]
fn validate_rejects_grpc_health_check_with_non_default_path() {
    let mut config = base_config();
    config.upstreams[0].peers[0].url = "https://example.com".to_string();
    config.upstreams[0].health_check_path = Some("/custom".to_string());
    config.upstreams[0].health_check_grpc_service = Some("grpc.health.v1.Health".to_string());

    let error = validate(&config).expect_err("custom gRPC health-check path should be rejected");
    assert!(error.to_string().contains(DEFAULT_GRPC_HEALTH_CHECK_PATH));
}

#[test]
fn validate_rejects_grpc_health_check_for_http1_upstream() {
    let mut config = base_config();
    config.upstreams[0].peers[0].url = "https://example.com".to_string();
    config.upstreams[0].protocol = UpstreamProtocolConfig::Http1;
    config.upstreams[0].health_check_grpc_service = Some("grpc.health.v1.Health".to_string());

    let error = validate(&config).expect_err("http1 gRPC health-check should be rejected");
    assert!(error.to_string().contains("requires protocol `Auto` or `Http2`"));
}

#[test]
fn validate_rejects_grpc_health_check_for_cleartext_peer() {
    let mut config = base_config();
    config.upstreams[0].health_check_grpc_service = Some("grpc.health.v1.Health".to_string());

    let error = validate(&config).expect_err("cleartext gRPC health-check peer should be rejected");
    assert!(error.to_string().contains("cleartext h2c health checks are not supported"));
}

#[test]
fn validate_rejects_invalid_health_check_path() {
    let mut config = base_config();
    config.upstreams[0].health_check_path = Some("healthz".to_string());

    let error = validate(&config).expect_err("invalid health check path should be rejected");
    assert!(error.to_string().contains("health_check_path must start with `/`"));
}

#[test]
fn validate_rejects_zero_health_check_interval() {
    let mut config = base_config();
    config.upstreams[0].health_check_path = Some("/healthz".to_string());
    config.upstreams[0].health_check_interval_secs = Some(0);

    let error = validate(&config).expect_err("zero health check interval should be rejected");
    assert!(error.to_string().contains("health_check_interval_secs must be greater than 0"));
}

#[test]
fn validate_rejects_zero_health_check_timeout() {
    let mut config = base_config();
    config.upstreams[0].health_check_path = Some("/healthz".to_string());
    config.upstreams[0].health_check_timeout_secs = Some(0);

    let error = validate(&config).expect_err("zero health check timeout should be rejected");
    assert!(error.to_string().contains("health_check_timeout_secs must be greater than 0"));
}

#[test]
fn validate_rejects_zero_peer_weight() {
    let mut config = base_config();
    config.upstreams[0].peers[0].weight = 0;

    let error = validate(&config).expect_err("zero peer weight should be rejected");
    assert!(error.to_string().contains("weight must be greater than 0"));
}

#[test]
fn validate_rejects_zero_healthy_successes_required() {
    let mut config = base_config();
    config.upstreams[0].health_check_path = Some("/healthz".to_string());
    config.upstreams[0].healthy_successes_required = Some(0);

    let error = validate(&config).expect_err("zero recovery threshold should be rejected");
    assert!(error.to_string().contains("healthy_successes_required must be greater than 0"));
}

#[test]
fn validate_rejects_http2_upstream_protocol_for_cleartext_peers() {
    let mut config = base_config();
    config.upstreams[0].protocol = UpstreamProtocolConfig::Http2;

    let error =
        validate(&config).expect_err("cleartext peers should be rejected for upstream http2");
    assert!(
        error
            .to_string()
            .contains("protocol `Http2` currently requires all peers to use `https://`")
    );
}

#[test]
fn validate_rejects_http3_upstream_protocol_for_cleartext_peers() {
    let mut config = base_config();
    config.upstreams[0].protocol = UpstreamProtocolConfig::Http3;

    let error =
        validate(&config).expect_err("cleartext peers should be rejected for upstream http3");
    assert!(
        error
            .to_string()
            .contains("protocol `Http3` currently requires all peers to use `https://`")
    );
}

#[test]
fn validate_rejects_invalid_http2_upstream_peer_uri() {
    let mut config = base_config();
    config.upstreams[0].protocol = UpstreamProtocolConfig::Http2;
    config.upstreams[0].peers[0].url = "not a uri".to_string();

    let error = validate(&config).expect_err("invalid http2 peer URI should be rejected");
    assert!(error.to_string().contains("peer url `not a uri` is not a valid URI"));
}

#[test]
fn validate_rejects_partial_upstream_mtls_identity() {
    let mut config = base_config();
    config.upstreams[0].tls = Some(crate::model::UpstreamTlsConfig {
        verify: crate::model::UpstreamTlsModeConfig::NativeRoots,
        versions: None,
        verify_depth: None,
        crl_path: None,
        client_cert_path: Some("client.crt".to_string()),
        client_key_path: None,
    });

    let error = validate(&config).expect_err("partial upstream mTLS identity should fail");
    assert!(error.to_string().contains("requires both client_cert_path and client_key_path"));
}

#[test]
fn validate_rejects_zero_upstream_verify_depth() {
    let mut config = base_config();
    config.upstreams[0].peers[0].url = "https://example.com".to_string();
    config.upstreams[0].tls = Some(crate::model::UpstreamTlsConfig {
        verify: crate::model::UpstreamTlsModeConfig::NativeRoots,
        versions: None,
        verify_depth: Some(0),
        crl_path: None,
        client_cert_path: None,
        client_key_path: None,
    });

    let error = validate(&config).expect_err("zero upstream verify_depth should fail");
    assert!(error.to_string().contains("verify_depth must be greater than 0"));
}

#[test]
fn validate_rejects_upstream_crl_when_verification_is_disabled() {
    let mut config = base_config();
    config.upstreams[0].peers[0].url = "https://example.com".to_string();
    config.upstreams[0].tls = Some(crate::model::UpstreamTlsConfig {
        verify: crate::model::UpstreamTlsModeConfig::Insecure,
        versions: None,
        verify_depth: None,
        crl_path: Some("revocations.pem".to_string()),
        client_cert_path: None,
        client_key_path: None,
    });

    let error = validate(&config).expect_err("upstream CRL should require verification");
    assert!(
        error.to_string().contains("verify_depth and crl_path require certificate verification")
    );
}

#[test]
fn validate_accepts_upstream_verify_depth_and_crl_with_custom_ca() {
    let mut config = base_config();
    config.upstreams[0].peers[0].url = "https://example.com".to_string();
    config.upstreams[0].tls = Some(crate::model::UpstreamTlsConfig {
        verify: crate::model::UpstreamTlsModeConfig::CustomCa {
            ca_cert_path: "upstream-ca.pem".to_string(),
        },
        versions: None,
        verify_depth: Some(2),
        crl_path: Some("upstream.crl.pem".to_string()),
        client_cert_path: None,
        client_key_path: None,
    });

    validate(&config).expect("upstream verify_depth and CRL should validate");
}

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

fn base_config() -> Config {
    Config {
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
            tls: None,
            http3: None,
        },
        upstreams: vec![UpstreamConfig {
            name: "backend".to_string(),
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
        }],
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Prefix("/".to_string()),
            handler: HandlerConfig::Proxy {
                upstream: "backend".to_string(),
                preserve_host: None,
                strip_prefix: None,
                proxy_set_headers: std::collections::HashMap::new(),
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
    }
}

fn valid_server_tls() -> ServerTlsConfig {
    ServerTlsConfig {
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
    }
}

fn sample_vhost(server_names: Vec<&str>) -> VirtualHostConfig {
    VirtualHostConfig {
        server_names: server_names.into_iter().map(str::to_string).collect(),
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Exact("/".to_string()),
            handler: HandlerConfig::Return {
                status: 200,
                location: String::new(),
                body: Some("vhost\n".to_string()),
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
        tls: None,
    }
}
