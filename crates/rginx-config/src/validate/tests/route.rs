use super::*;

#[test]
fn validate_rejects_invalid_route_allow_cidr() {
    let mut config = base_config();
    config.locations[0].allow_cidrs = vec!["not-a-cidr".to_string()];

    let error = validate(&config).expect_err("invalid CIDR should be rejected");
    assert!(error.to_string().contains("allow_cidrs entry `not-a-cidr` is invalid"));
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
fn validate_rejects_invalid_regex_route_matcher() {
    let mut config = base_config();
    config.locations[0].matcher =
        MatcherConfig::Regex { pattern: "(".to_string(), case_insensitive: false };

    let error = validate(&config).expect_err("invalid regex matcher should be rejected");
    assert!(error.to_string().contains("route regex pattern `(` is invalid"));
}

#[test]
fn validate_rejects_invalid_dynamic_proxy_header_template() {
    let mut config = base_config();
    let HandlerConfig::Proxy { proxy_set_headers, .. } = &mut config.locations[0].handler else {
        panic!("base route should proxy");
    };
    proxy_set_headers.insert(
        "origin".to_string(),
        ProxyHeaderValueConfig::Dynamic(ProxyHeaderDynamicValueConfig::Template(
            "https://{missing}".to_string(),
        )),
    );

    let error = validate(&config).expect_err("invalid proxy header template should be rejected");
    assert!(error.to_string().contains("proxy_set_headers Template for `origin` is invalid"));
}

#[test]
fn validate_allows_duplicate_exact_routes_when_grpc_constraints_differ() {
    let mut config = base_config();
    config.locations[0].matcher = MatcherConfig::Exact("/grpc.health.v1.Health/Check".to_string());
    config.locations.push(LocationConfig {
        cache: None,
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
