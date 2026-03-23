use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub runtime: RuntimeConfig,
    pub server: ServerConfig,
    #[serde(default)]
    pub upstreams: Vec<UpstreamConfig>,
    pub locations: Vec<LocationConfig>,
    #[serde(default)]
    pub servers: Vec<VirtualHostConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeConfig {
    pub shutdown_timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub listen: String,
    #[serde(default)]
    pub server_names: Vec<String>,
    #[serde(default)]
    pub trusted_proxies: Vec<String>,
    #[serde(default)]
    pub keep_alive: Option<bool>,
    #[serde(default)]
    pub max_headers: Option<u64>,
    #[serde(default)]
    pub max_request_body_bytes: Option<u64>,
    #[serde(default)]
    pub max_connections: Option<u64>,
    #[serde(default)]
    pub header_read_timeout_secs: Option<u64>,
    #[serde(default)]
    pub tls: Option<ServerTlsConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerTlsConfig {
    pub cert_path: String,
    pub key_path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpstreamConfig {
    pub name: String,
    pub peers: Vec<UpstreamPeerConfig>,
    pub tls: Option<UpstreamTlsConfig>,
    #[serde(default)]
    pub protocol: UpstreamProtocolConfig,
    #[serde(default)]
    pub load_balance: UpstreamLoadBalanceConfig,
    #[serde(default)]
    pub server_name_override: Option<String>,
    #[serde(default)]
    pub request_timeout_secs: Option<u64>,
    #[serde(default)]
    pub connect_timeout_secs: Option<u64>,
    #[serde(default)]
    pub read_timeout_secs: Option<u64>,
    #[serde(default)]
    pub write_timeout_secs: Option<u64>,
    #[serde(default)]
    pub idle_timeout_secs: Option<u64>,
    #[serde(default)]
    pub pool_idle_timeout_secs: Option<u64>,
    #[serde(default)]
    pub pool_max_idle_per_host: Option<u64>,
    #[serde(default)]
    pub tcp_keepalive_secs: Option<u64>,
    #[serde(default)]
    pub tcp_nodelay: Option<bool>,
    #[serde(default)]
    pub http2_keep_alive_interval_secs: Option<u64>,
    #[serde(default)]
    pub http2_keep_alive_timeout_secs: Option<u64>,
    #[serde(default)]
    pub http2_keep_alive_while_idle: Option<bool>,
    #[serde(default)]
    pub max_replayable_request_body_bytes: Option<u64>,
    #[serde(default)]
    pub unhealthy_after_failures: Option<u32>,
    #[serde(default)]
    pub unhealthy_cooldown_secs: Option<u64>,
    #[serde(default)]
    pub health_check_path: Option<String>,
    #[serde(default)]
    pub health_check_interval_secs: Option<u64>,
    #[serde(default)]
    pub health_check_timeout_secs: Option<u64>,
    #[serde(default)]
    pub healthy_successes_required: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpstreamPeerConfig {
    pub url: String,
    #[serde(default = "default_upstream_peer_weight")]
    pub weight: u32,
    #[serde(default)]
    pub backup: bool,
}

const fn default_upstream_peer_weight() -> u32 {
    1
}

#[derive(Debug, Clone, Deserialize)]
pub enum UpstreamTlsConfig {
    NativeRoots,
    CustomCa { ca_cert_path: String },
    Insecure,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub enum UpstreamProtocolConfig {
    #[default]
    Auto,
    Http1,
    Http2,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub enum UpstreamLoadBalanceConfig {
    #[default]
    RoundRobin,
    IpHash,
    LeastConn,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LocationConfig {
    pub matcher: MatcherConfig,
    pub handler: HandlerConfig,
    #[serde(default)]
    pub allow_cidrs: Vec<String>,
    #[serde(default)]
    pub deny_cidrs: Vec<String>,
    #[serde(default)]
    pub requests_per_sec: Option<u32>,
    #[serde(default)]
    pub burst: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub enum MatcherConfig {
    Exact(String),
    Prefix(String),
}

#[derive(Debug, Clone, Deserialize)]
pub enum HandlerConfig {
    Static {
        status: Option<u16>,
        content_type: Option<String>,
        body: String,
    },
    Proxy {
        upstream: String,
        #[serde(default)]
        preserve_host: Option<bool>,
        #[serde(default)]
        strip_prefix: Option<String>,
        #[serde(default)]
        proxy_set_headers: HashMap<String, String>,
    },
    File {
        root: String,
        #[serde(default)]
        index: Option<String>,
        #[serde(default)]
        try_files: Option<Vec<String>>,
    },
    Return {
        status: u16,
        location: String,
        #[serde(default)]
        body: Option<String>,
    },
    Status,
    Metrics,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VirtualHostConfig {
    #[serde(default)]
    pub server_names: Vec<String>,
    pub locations: Vec<LocationConfig>,
    #[serde(default)]
    pub tls: Option<ServerTlsConfig>,
}
