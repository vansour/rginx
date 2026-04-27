use serde::{Deserialize, Deserializer};

use super::{Http3Config, ServerTlsConfig};

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub listen: Option<String>,
    pub server_header: Option<String>,
    pub proxy_protocol: Option<bool>,
    pub default_certificate: Option<String>,
    pub server_names: Vec<String>,
    pub trusted_proxies: Vec<String>,
    pub client_ip_header: Option<String>,
    pub keep_alive: Option<bool>,
    pub max_headers: Option<u64>,
    pub max_request_body_bytes: Option<u64>,
    pub max_connections: Option<u64>,
    pub header_read_timeout_secs: Option<u64>,
    pub request_body_read_timeout_secs: Option<u64>,
    pub response_write_timeout_secs: Option<u64>,
    pub access_log_format: Option<String>,
    pub tls: Option<ServerTlsConfig>,
    pub http3: Option<Http3Config>,
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
            server_header: Option<String>,
            #[serde(default)]
            proxy_protocol: Option<bool>,
            #[serde(default)]
            default_certificate: Option<String>,
            #[serde(default)]
            server_names: Vec<String>,
            #[serde(default)]
            trusted_proxies: Vec<String>,
            #[serde(default)]
            client_ip_header: Option<String>,
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
            #[serde(default)]
            http3: Option<Http3Config>,
        }

        let server = ServerConfigDe::deserialize(deserializer)?;
        Ok(ServerConfig {
            listen: server.listen.0,
            server_header: server.server_header,
            proxy_protocol: server.proxy_protocol,
            default_certificate: server.default_certificate,
            server_names: server.server_names,
            trusted_proxies: server.trusted_proxies,
            client_ip_header: server.client_ip_header,
            keep_alive: server.keep_alive,
            max_headers: server.max_headers,
            max_request_body_bytes: server.max_request_body_bytes,
            max_connections: server.max_connections,
            header_read_timeout_secs: server.header_read_timeout_secs,
            request_body_read_timeout_secs: server.request_body_read_timeout_secs,
            response_write_timeout_secs: server.response_write_timeout_secs,
            access_log_format: server.access_log_format,
            tls: server.tls,
            http3: server.http3,
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
