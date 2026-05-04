use super::*;

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
fn validate_rejects_zero_peer_weight() {
    let mut config = base_config();
    config.upstreams[0].peers[0].weight = 0;

    let error = validate(&config).expect_err("zero peer weight should be rejected");
    assert!(error.to_string().contains("weight must be greater than 0"));
}

#[test]
fn validate_rejects_zero_peer_max_conns() {
    let mut config = base_config();
    config.upstreams[0].peers[0].max_conns = Some(0);

    let error = validate(&config).expect_err("zero peer max_conns should be rejected");
    assert!(error.to_string().contains("max_conns must be greater than 0"));
}
