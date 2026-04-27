use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
pub enum RouteBufferingPolicyConfig {
    #[default]
    Auto,
    On,
    Off,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
pub enum RouteCompressionPolicyConfig {
    Off,
    #[default]
    Auto,
    Force,
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
    #[serde(default)]
    pub allow_early_data: Option<bool>,
    #[serde(default)]
    pub request_buffering: Option<RouteBufferingPolicyConfig>,
    #[serde(default)]
    pub response_buffering: Option<RouteBufferingPolicyConfig>,
    #[serde(default)]
    pub compression: Option<RouteCompressionPolicyConfig>,
    #[serde(default)]
    pub compression_min_bytes: Option<u64>,
    #[serde(default)]
    pub compression_content_types: Option<Vec<String>>,
    #[serde(default)]
    pub streaming_response_idle_timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub enum MatcherConfig {
    Exact(String),
    Prefix(String),
    Regex {
        pattern: String,
        #[serde(default)]
        case_insensitive: bool,
    },
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
        proxy_set_headers: HashMap<String, ProxyHeaderValueConfig>,
    },
    Return {
        status: u16,
        location: String,
        #[serde(default)]
        body: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ProxyHeaderValueConfig {
    Dynamic(ProxyHeaderDynamicValueConfig),
    Static(String),
}

#[derive(Debug, Clone, Deserialize)]
pub enum ProxyHeaderDynamicValueConfig {
    Host,
    Scheme,
    ClientIp,
    RemoteAddr,
    PeerAddr,
    ForwardedFor,
    RequestHeader(String),
    Template(String),
}
