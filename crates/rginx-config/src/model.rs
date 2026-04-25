use std::collections::HashMap;

use serde::{Deserialize, Deserializer, de};

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

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub listen: Option<String>,
    pub server_header: Option<String>,
    pub proxy_protocol: Option<bool>,
    pub default_certificate: Option<String>,
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
    pub http3: Option<Http3Config>,
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

#[derive(Debug, Clone, Deserialize)]
pub struct ServerTlsConfig {
    pub cert_path: String,
    pub key_path: String,
    #[serde(default)]
    pub additional_certificates: Option<Vec<ServerCertificateBundleConfig>>,
    #[serde(default)]
    pub versions: Option<Vec<TlsVersionConfig>>,
    #[serde(default)]
    pub cipher_suites: Option<Vec<TlsCipherSuiteConfig>>,
    #[serde(default)]
    pub key_exchange_groups: Option<Vec<TlsKeyExchangeGroupConfig>>,
    #[serde(default)]
    pub alpn_protocols: Option<Vec<String>>,
    #[serde(default)]
    pub ocsp_staple_path: Option<String>,
    #[serde(default)]
    pub ocsp: Option<OcspConfig>,
    #[serde(default)]
    pub session_resumption: Option<bool>,
    #[serde(default)]
    pub session_tickets: Option<bool>,
    #[serde(default)]
    pub session_cache_size: Option<u64>,
    #[serde(default)]
    pub session_ticket_count: Option<u64>,
    #[serde(default)]
    pub client_auth: Option<ServerClientAuthConfig>,
}

#[derive(Debug, Clone)]
pub struct VirtualHostTlsConfig {
    pub cert_path: String,
    pub key_path: String,
    pub additional_certificates: Option<Vec<ServerCertificateBundleConfig>>,
    pub ocsp_staple_path: Option<String>,
    pub ocsp: Option<OcspConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerCertificateBundleConfig {
    pub cert_path: String,
    pub key_path: String,
    #[serde(default)]
    pub ocsp_staple_path: Option<String>,
    #[serde(default)]
    pub ocsp: Option<OcspConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OcspConfig {
    #[serde(default)]
    pub nonce: Option<OcspNonceModeConfig>,
    #[serde(default)]
    pub responder_policy: Option<OcspResponderPolicyConfig>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Hash)]
pub enum OcspNonceModeConfig {
    Disabled,
    Preferred,
    Required,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Hash)]
pub enum OcspResponderPolicyConfig {
    IssuerOnly,
    IssuerOrDelegated,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Hash)]
pub enum TlsVersionConfig {
    Tls12,
    Tls13,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Hash)]
pub enum TlsCipherSuiteConfig {
    Tls13Aes256GcmSha384,
    Tls13Aes128GcmSha256,
    Tls13Chacha20Poly1305Sha256,
    TlsEcdheEcdsaWithAes256GcmSha384,
    TlsEcdheEcdsaWithAes128GcmSha256,
    TlsEcdheEcdsaWithChacha20Poly1305Sha256,
    TlsEcdheRsaWithAes256GcmSha384,
    TlsEcdheRsaWithAes128GcmSha256,
    TlsEcdheRsaWithChacha20Poly1305Sha256,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Hash)]
pub enum TlsKeyExchangeGroupConfig {
    X25519,
    Secp256r1,
    Secp384r1,
    X25519Mlkem768,
    Secp256r1Mlkem768,
    Mlkem768,
    Mlkem1024,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub enum ServerClientAuthModeConfig {
    Optional,
    Required,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerClientAuthConfig {
    pub mode: ServerClientAuthModeConfig,
    pub ca_cert_path: String,
    #[serde(default)]
    pub verify_depth: Option<u32>,
    #[serde(default)]
    pub crl_path: Option<String>,
}

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
    Http3,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub enum UpstreamLoadBalanceConfig {
    #[default]
    RoundRobin,
    IpHash,
    LeastConn,
}

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
    pub tls: Option<VirtualHostTlsConfig>,
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

impl<'de> Deserialize<'de> for VirtualHostTlsConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Debug, Clone, Deserialize)]
        enum VirtualHostTlsConfigDe {
            VirtualHostTlsConfig {
                cert_path: String,
                key_path: String,
                #[serde(default)]
                additional_certificates: Option<Vec<ServerCertificateBundleConfig>>,
                #[serde(default)]
                ocsp_staple_path: Option<String>,
                #[serde(default)]
                ocsp: Option<OcspConfig>,
            },
            ServerTlsConfig {
                cert_path: String,
                key_path: String,
                #[serde(default)]
                additional_certificates: Option<Vec<ServerCertificateBundleConfig>>,
                #[serde(default)]
                versions: Option<Vec<TlsVersionConfig>>,
                #[serde(default)]
                cipher_suites: Option<Vec<TlsCipherSuiteConfig>>,
                #[serde(default)]
                key_exchange_groups: Option<Vec<TlsKeyExchangeGroupConfig>>,
                #[serde(default)]
                alpn_protocols: Option<Vec<String>>,
                #[serde(default)]
                ocsp_staple_path: Option<String>,
                #[serde(default)]
                ocsp: Option<OcspConfig>,
                #[serde(default)]
                session_resumption: Option<bool>,
                #[serde(default)]
                session_tickets: Option<bool>,
                #[serde(default)]
                session_cache_size: Option<u64>,
                #[serde(default)]
                session_ticket_count: Option<u64>,
                #[serde(default)]
                client_auth: Option<ServerClientAuthConfig>,
            },
        }

        match VirtualHostTlsConfigDe::deserialize(deserializer)? {
            VirtualHostTlsConfigDe::VirtualHostTlsConfig {
                cert_path,
                key_path,
                additional_certificates,
                ocsp_staple_path,
                ocsp,
            } => Ok(Self { cert_path, key_path, additional_certificates, ocsp_staple_path, ocsp }),
            VirtualHostTlsConfigDe::ServerTlsConfig {
                cert_path,
                key_path,
                additional_certificates,
                versions,
                cipher_suites,
                key_exchange_groups,
                alpn_protocols,
                ocsp_staple_path,
                ocsp,
                session_resumption,
                session_tickets,
                session_cache_size,
                session_ticket_count,
                client_auth,
            } => {
                if versions.is_some()
                    || cipher_suites.is_some()
                    || key_exchange_groups.is_some()
                    || alpn_protocols.is_some()
                    || session_resumption.is_some()
                    || session_tickets.is_some()
                    || session_cache_size.is_some()
                    || session_ticket_count.is_some()
                    || client_auth.is_some()
                {
                    return Err(de::Error::custom(
                        "vhost TLS policy fields are not supported in legacy `ServerTlsConfig(...)`; use `VirtualHostTlsConfig(...)` for certificate overrides and keep versions, cipher_suites, key_exchange_groups, ALPN, session settings, session cache settings, and client_auth on server.tls or listeners[].tls",
                    ));
                }

                Ok(Self { cert_path, key_path, additional_certificates, ocsp_staple_path, ocsp })
            }
        }
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
