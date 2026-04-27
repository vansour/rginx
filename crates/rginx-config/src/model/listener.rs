use serde::Deserialize;

use super::ServerTlsConfig;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Http3Config {
    #[serde(default)]
    pub listen: Option<String>,
    #[serde(default)]
    pub advertise_alt_svc: Option<bool>,
    #[serde(default)]
    pub alt_svc_max_age_secs: Option<u64>,
    #[serde(default)]
    pub max_concurrent_streams: Option<u64>,
    #[serde(default)]
    pub stream_buffer_size_bytes: Option<u64>,
    #[serde(default)]
    pub active_connection_id_limit: Option<u32>,
    #[serde(default)]
    pub retry: Option<bool>,
    #[serde(default)]
    pub host_key_path: Option<String>,
    #[serde(default)]
    pub gso: Option<bool>,
    #[serde(default)]
    pub early_data: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListenerConfig {
    pub name: String,
    pub listen: String,
    #[serde(default)]
    pub server_header: Option<String>,
    #[serde(default)]
    pub proxy_protocol: Option<bool>,
    #[serde(default)]
    pub default_certificate: Option<String>,
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
    #[serde(default)]
    pub http3: Option<Http3Config>,
}
