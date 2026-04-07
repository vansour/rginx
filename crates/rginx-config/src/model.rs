use std::collections::HashMap;

use serde::{Deserialize, Deserializer};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub listeners: Vec<ListenerConfig>,
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
    #[serde(default)]
    pub worker_threads: Option<u64>,
    #[serde(default)]
    pub accept_workers: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub listen: Option<String>,
    pub proxy_protocol: Option<bool>,
    pub server_names: Vec<String>,
    pub trusted_proxies: Vec<String>,
    pub keep_alive: Option<bool>,
    pub max_headers: Option<u64>,
    pub max_request_body_bytes: Option<u64>,
    pub max_connections: Option<u64>,
    pub header_read_timeout_secs: Option<u64>,
    pub request_body_read_timeout_secs: Option<u64>,
    pub response_write_timeout_secs: Option<u64>,
    pub access_log_format: Option<String>,
    pub tls: Option<ServerTlsConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListenerConfig {
    pub name: String,
    pub listen: String,
    #[serde(default)]
    pub proxy_protocol: Option<bool>,
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
    pub request_body_read_timeout_secs: Option<u64>,
    #[serde(default)]
    pub response_write_timeout_secs: Option<u64>,
    #[serde(default)]
    pub access_log_format: Option<String>,
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
    pub health_check_grpc_service: Option<String>,
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
    pub grpc_service: Option<String>,
    #[serde(default)]
    pub grpc_method: Option<String>,
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
    Proxy {
        upstream: String,
        #[serde(default)]
        preserve_host: Option<bool>,
        #[serde(default)]
        strip_prefix: Option<String>,
        #[serde(default)]
        proxy_set_headers: HashMap<String, String>,
    },
    Return {
        status: u16,
        location: String,
        #[serde(default)]
        body: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct VirtualHostConfig {
    #[serde(default)]
    pub server_names: Vec<String>,
    pub locations: Vec<LocationConfig>,
    #[serde(default)]
    pub tls: Option<ServerTlsConfig>,
}

impl<'de> Deserialize<'de> for ServerConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename = "ServerConfig")]
        struct ServerConfigDe {
            #[serde(default)]
            listen: MaybeString,
            #[serde(default)]
            proxy_protocol: Option<bool>,
            #[serde(default)]
            server_names: Vec<String>,
            #[serde(default)]
            trusted_proxies: Vec<String>,
            #[serde(default)]
            keep_alive: Option<bool>,
            #[serde(default)]
            max_headers: Option<u64>,
            #[serde(default)]
            max_request_body_bytes: Option<u64>,
            #[serde(default)]
            max_connections: Option<u64>,
            #[serde(default)]
            header_read_timeout_secs: Option<u64>,
            #[serde(default)]
            request_body_read_timeout_secs: Option<u64>,
            #[serde(default)]
            response_write_timeout_secs: Option<u64>,
            #[serde(default)]
            access_log_format: Option<String>,
            #[serde(default)]
            tls: Option<ServerTlsConfig>,
        }

        let server = ServerConfigDe::deserialize(deserializer)?;
        Ok(ServerConfig {
            listen: server.listen.0,
            proxy_protocol: server.proxy_protocol,
            server_names: server.server_names,
            trusted_proxies: server.trusted_proxies,
            keep_alive: server.keep_alive,
            max_headers: server.max_headers,
            max_request_body_bytes: server.max_request_body_bytes,
            max_connections: server.max_connections,
            header_read_timeout_secs: server.header_read_timeout_secs,
            request_body_read_timeout_secs: server.request_body_read_timeout_secs,
            response_write_timeout_secs: server.response_write_timeout_secs,
            access_log_format: server.access_log_format,
            tls: server.tls,
        })
    }
}

#[derive(Debug, Clone, Default)]
struct MaybeString(Option<String>);

impl<'de> Deserialize<'de> for MaybeString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StringOrOption {
            String(String),
            Option(Option<String>),
        }

        Ok(match StringOrOption::deserialize(deserializer)? {
            StringOrOption::String(value) => Self(Some(value)),
            StringOrOption::Option(value) => Self(value),
        })
    }
}
