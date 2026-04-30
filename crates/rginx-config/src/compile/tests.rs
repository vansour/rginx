use std::fs;
use std::time::Duration;

use crate::model::{
    CacheRouteConfig, CacheZoneConfig, Config, HandlerConfig, Http3Config, ListenerConfig,
    LocationConfig, MatcherConfig, ProxyHeaderDynamicValueConfig, ProxyHeaderValueConfig,
    RouteBufferingPolicyConfig, RouteCompressionPolicyConfig, RuntimeConfig, ServerConfig,
    ServerTlsConfig, TlsCipherSuiteConfig, TlsKeyExchangeGroupConfig, UpstreamConfig,
    UpstreamLoadBalanceConfig, UpstreamPeerConfig, UpstreamProtocolConfig, UpstreamTlsConfig,
    VirtualHostConfig,
};
use tempfile::TempDir;

use super::{
    DEFAULT_GRPC_HEALTH_CHECK_PATH, DEFAULT_HEALTH_CHECK_INTERVAL_SECS,
    DEFAULT_HEALTH_CHECK_TIMEOUT_SECS, DEFAULT_HEALTHY_SUCCESSES_REQUIRED,
    DEFAULT_MAX_REPLAYABLE_REQUEST_BODY_BYTES, DEFAULT_UNHEALTHY_AFTER_FAILURES,
    DEFAULT_UNHEALTHY_COOLDOWN_SECS, DEFAULT_UPSTREAM_HTTP2_KEEP_ALIVE_TIMEOUT_SECS,
    DEFAULT_UPSTREAM_POOL_IDLE_TIMEOUT_SECS, DEFAULT_UPSTREAM_REQUEST_TIMEOUT_SECS, compile,
    compile_with_base, server,
};

fn default_listener_server(snapshot: &rginx_core::ConfigSnapshot) -> &rginx_core::Server {
    &snapshot
        .listeners
        .first()
        .expect("compiled snapshot should contain at least one listener")
        .server
}

fn temp_base_dir(prefix: &str) -> TempDir {
    tempfile::Builder::new().prefix(prefix).tempdir().expect("temp base dir should be created")
}

fn test_location(matcher: MatcherConfig, handler: HandlerConfig) -> LocationConfig {
    LocationConfig {
        cache: None,
        matcher,
        handler,
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
    }
}

mod acme;
mod cache;
mod cache_p1;
mod cache_p2;
mod cache_p3;
mod http3;
mod listeners;
mod route;
mod server_settings;
mod server_tls;
mod upstream_defaults;
mod upstream_fallbacks;
mod upstream_server_name;
mod upstream_tls;
mod upstream_transport;
mod vhosts;
