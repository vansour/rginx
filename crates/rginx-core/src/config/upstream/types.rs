use super::*;

#[derive(Debug, Clone)]
pub struct ActiveHealthCheck {
    pub path: String,
    pub grpc_service: Option<String>,
    pub interval: Duration,
    pub timeout: Duration,
    pub healthy_successes_required: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpstreamProtocol {
    Auto,
    Http1,
    Http2,
    H2c,
    Http3,
}

impl UpstreamProtocol {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Http1 => "http1",
            Self::Http2 => "http2",
            Self::H2c => "h2c",
            Self::Http3 => "http3",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpstreamLoadBalance {
    RoundRobin,
    IpHash,
    LeastConn,
}

impl UpstreamLoadBalance {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RoundRobin => "round_robin",
            Self::IpHash => "ip_hash",
            Self::LeastConn => "least_conn",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UpstreamDnsPolicy {
    pub resolver_addrs: Vec<SocketAddr>,
    pub min_ttl: Duration,
    pub max_ttl: Duration,
    pub negative_ttl: Duration,
    pub stale_if_error: Duration,
    pub refresh_before_expiry: Duration,
    pub prefer_ipv4: bool,
    pub prefer_ipv6: bool,
}

impl Default for UpstreamDnsPolicy {
    fn default() -> Self {
        Self {
            resolver_addrs: Vec::new(),
            min_ttl: Duration::from_secs(5),
            max_ttl: Duration::from_secs(300),
            negative_ttl: Duration::from_secs(30),
            stale_if_error: Duration::from_secs(60),
            refresh_before_expiry: Duration::from_secs(10),
            prefer_ipv4: false,
            prefer_ipv6: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct UpstreamSettings {
    pub protocol: UpstreamProtocol,
    pub load_balance: UpstreamLoadBalance,
    pub dns: UpstreamDnsPolicy,
    pub server_name: bool,
    pub server_name_override: Option<String>,
    pub tls_versions: Option<Vec<TlsVersion>>,
    pub server_verify_depth: Option<u32>,
    pub server_crl_path: Option<PathBuf>,
    pub client_identity: Option<ClientIdentity>,
    pub request_timeout: Duration,
    pub connect_timeout: Duration,
    pub write_timeout: Duration,
    pub idle_timeout: Duration,
    pub pool_idle_timeout: Option<Duration>,
    pub pool_max_idle_per_host: usize,
    pub tcp_keepalive: Option<Duration>,
    pub tcp_nodelay: bool,
    pub http2_keep_alive_interval: Option<Duration>,
    pub http2_keep_alive_timeout: Duration,
    pub http2_keep_alive_while_idle: bool,
    pub max_replayable_request_body_bytes: usize,
    pub unhealthy_after_failures: u32,
    pub unhealthy_cooldown: Duration,
    pub active_health_check: Option<ActiveHealthCheck>,
}

#[derive(Debug)]
pub struct Upstream {
    pub name: String,
    pub peers: Vec<UpstreamPeer>,
    pub tls: UpstreamTls,
    pub protocol: UpstreamProtocol,
    pub load_balance: UpstreamLoadBalance,
    pub dns: UpstreamDnsPolicy,
    pub server_name: bool,
    pub server_name_override: Option<String>,
    pub tls_versions: Option<Vec<TlsVersion>>,
    pub server_verify_depth: Option<u32>,
    pub server_crl_path: Option<PathBuf>,
    pub client_identity: Option<ClientIdentity>,
    pub request_timeout: Duration,
    pub connect_timeout: Duration,
    pub write_timeout: Duration,
    pub idle_timeout: Duration,
    pub pool_idle_timeout: Option<Duration>,
    pub pool_max_idle_per_host: usize,
    pub tcp_keepalive: Option<Duration>,
    pub tcp_nodelay: bool,
    pub http2_keep_alive_interval: Option<Duration>,
    pub http2_keep_alive_timeout: Duration,
    pub http2_keep_alive_while_idle: bool,
    pub max_replayable_request_body_bytes: usize,
    pub unhealthy_after_failures: u32,
    pub unhealthy_cooldown: Duration,
    pub active_health_check: Option<ActiveHealthCheck>,
    pub(super) cursor: AtomicUsize,
}

#[derive(Debug, Clone)]
pub struct UpstreamPeer {
    pub url: String,
    pub scheme: String,
    pub authority: String,
    pub weight: u32,
    pub backup: bool,
    pub max_conns: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UpstreamTls {
    NativeRoots,
    CustomCa { ca_cert_path: PathBuf },
    Insecure,
}
