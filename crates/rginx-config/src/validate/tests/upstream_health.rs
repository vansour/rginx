use super::*;

#[test]
fn validate_rejects_empty_grpc_service() {
    let mut config = base_config();
    config.locations[0].grpc_service = Some("   ".to_string());

    let error = validate(&config).expect_err("empty grpc_service should be rejected");
    assert!(error.to_string().contains("grpc_service must not be empty"));
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
