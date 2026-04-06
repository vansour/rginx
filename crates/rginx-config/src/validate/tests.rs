use crate::model::{
    Config, HandlerConfig, LocationConfig, MatcherConfig, RuntimeConfig, ServerConfig,
    ServerTlsConfig, UpstreamConfig, UpstreamLoadBalanceConfig, UpstreamPeerConfig,
    UpstreamProtocolConfig, VirtualHostConfig,
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
    config.server.tls =
        Some(ServerTlsConfig { cert_path: " ".to_string(), key_path: "server.key".to_string() });

    let error = validate(&config).expect_err("empty cert path should be rejected");
    assert!(error.to_string().contains("server TLS certificate path must not be empty"));
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
    vhost.tls = Some(ServerTlsConfig {
        cert_path: "server.crt".to_string(),
        key_path: "server.key".to_string(),
    });
    config.servers = vec![vhost];

    let error = validate(&config).expect_err("TLS vhost without server_name should be rejected");
    assert!(error.to_string().contains("servers[0] TLS requires at least one server_name"));
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
fn validate_rejects_invalid_http2_upstream_peer_uri() {
    let mut config = base_config();
    config.upstreams[0].protocol = UpstreamProtocolConfig::Http2;
    config.upstreams[0].peers[0].url = "not a uri".to_string();

    let error = validate(&config).expect_err("invalid http2 peer URI should be rejected");
    assert!(error.to_string().contains("peer url `not a uri` is not a valid URI"));
}

fn base_config() -> Config {
    Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        server: ServerConfig {
            listen: "127.0.0.1:8080".to_string(),
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
        },
        upstreams: vec![UpstreamConfig {
            name: "backend".to_string(),
            peers: vec![UpstreamPeerConfig {
                url: "http://127.0.0.1:9000".to_string(),
                weight: 1,
                backup: false,
            }],
            tls: None,
            protocol: UpstreamProtocolConfig::Auto,
            load_balance: UpstreamLoadBalanceConfig::RoundRobin,
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
        }],
        servers: Vec::new(),
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
        }],
        tls: None,
    }
}
