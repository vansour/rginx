use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub runtime: RuntimeConfig,
    pub server: ServerConfig,
    #[serde(default)]
    pub upstreams: Vec<UpstreamConfig>,
    pub locations: Vec<LocationConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeConfig {
    pub shutdown_timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub listen: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpstreamConfig {
    pub name: String,
    pub peers: Vec<UpstreamPeerConfig>,
    pub tls: Option<UpstreamTlsConfig>,
    #[serde(default)]
    pub server_name_override: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpstreamPeerConfig {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub enum UpstreamTlsConfig {
    NativeRoots,
    CustomCa { ca_cert_path: String },
    Insecure,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LocationConfig {
    pub matcher: MatcherConfig,
    pub handler: HandlerConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub enum MatcherConfig {
    Exact(String),
    Prefix(String),
}

#[derive(Debug, Clone, Deserialize)]
pub enum HandlerConfig {
    Static { status: Option<u16>, content_type: Option<String>, body: String },
    Proxy { upstream: String },
}
