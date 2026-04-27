use serde::{Deserialize, Deserializer};

use super::TlsVersionConfig;

#[derive(Debug, Clone, Deserialize)]
pub struct UpstreamConfig {
    pub name: String,
    pub peers: Vec<UpstreamPeerConfig>,
    pub tls: Option<UpstreamTlsConfig>,
    #[serde(default)]
    pub dns: Option<UpstreamDnsConfig>,
    #[serde(default)]
    pub protocol: UpstreamProtocolConfig,
    #[serde(default)]
    pub load_balance: UpstreamLoadBalanceConfig,
    #[serde(default)]
    pub server_name: Option<bool>,
    #[serde(default, alias = "tls_server_name")]
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

#[derive(Debug, Clone, Deserialize, Default)]
pub struct UpstreamDnsConfig {
    #[serde(default)]
    pub resolver_addrs: Vec<String>,
    #[serde(default)]
    pub min_ttl_secs: Option<u64>,
    #[serde(default)]
    pub max_ttl_secs: Option<u64>,
    #[serde(default)]
    pub negative_ttl_secs: Option<u64>,
    #[serde(default)]
    pub stale_if_error_secs: Option<u64>,
    #[serde(default)]
    pub refresh_before_expiry_secs: Option<u64>,
    #[serde(default)]
    pub prefer_ipv4: Option<bool>,
    #[serde(default)]
    pub prefer_ipv6: Option<bool>,
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

#[derive(Debug, Clone, Deserialize, Default)]
pub enum UpstreamTlsModeConfig {
    #[default]
    NativeRoots,
    CustomCa {
        ca_cert_path: String,
    },
    Insecure,
}

#[derive(Debug, Clone)]
pub struct UpstreamTlsConfig {
    pub verify: UpstreamTlsModeConfig,
    pub versions: Option<Vec<TlsVersionConfig>>,
    pub verify_depth: Option<u32>,
    pub crl_path: Option<String>,
    pub client_cert_path: Option<String>,
    pub client_key_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub enum UpstreamProtocolConfig {
    #[default]
    Auto,
    Http1,
    Http2,
    H2c,
    Http3,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub enum UpstreamLoadBalanceConfig {
    #[default]
    RoundRobin,
    IpHash,
    LeastConn,
}

impl<'de> Deserialize<'de> for UpstreamTlsConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Debug, Clone, Deserialize)]
        enum UpstreamTlsConfigDe {
            NativeRoots,
            CustomCa {
                ca_cert_path: String,
            },
            Insecure,
            UpstreamTlsConfig {
                #[serde(default, alias = "verify")]
                verify: UpstreamTlsModeConfig,
                #[serde(default)]
                versions: Option<Vec<TlsVersionConfig>>,
                #[serde(default)]
                verify_depth: Option<u32>,
                #[serde(default)]
                crl_path: Option<String>,
                #[serde(default)]
                client_cert_path: Option<String>,
                #[serde(default)]
                client_key_path: Option<String>,
            },
        }

        Ok(match UpstreamTlsConfigDe::deserialize(deserializer)? {
            UpstreamTlsConfigDe::NativeRoots => Self {
                verify: UpstreamTlsModeConfig::NativeRoots,
                versions: None,
                verify_depth: None,
                crl_path: None,
                client_cert_path: None,
                client_key_path: None,
            },
            UpstreamTlsConfigDe::CustomCa { ca_cert_path } => Self {
                verify: UpstreamTlsModeConfig::CustomCa { ca_cert_path },
                versions: None,
                verify_depth: None,
                crl_path: None,
                client_cert_path: None,
                client_key_path: None,
            },
            UpstreamTlsConfigDe::Insecure => Self {
                verify: UpstreamTlsModeConfig::Insecure,
                versions: None,
                verify_depth: None,
                crl_path: None,
                client_cert_path: None,
                client_key_path: None,
            },
            UpstreamTlsConfigDe::UpstreamTlsConfig {
                verify,
                versions,
                verify_depth,
                crl_path,
                client_cert_path,
                client_key_path,
            } => {
                Self { verify, versions, verify_depth, crl_path, client_cert_path, client_key_path }
            }
        })
    }
}
