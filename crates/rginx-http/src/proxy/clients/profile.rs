use std::path::PathBuf;

use rginx_core::{ClientIdentity, TlsVersion};

use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct UpstreamClientProfile {
    pub(super) tls: UpstreamTls,
    pub(super) dns: rginx_core::UpstreamDnsPolicy,
    pub(super) tls_versions: Option<Vec<TlsVersion>>,
    pub(super) server_verify_depth: Option<u32>,
    pub(super) server_crl_path: Option<PathBuf>,
    pub(super) client_identity: Option<ClientIdentity>,
    pub(super) protocol: UpstreamProtocol,
    pub(super) server_name: bool,
    pub(super) server_name_override: Option<String>,
    pub(super) connect_timeout: Duration,
    pub(super) pool_idle_timeout: Option<Duration>,
    pub(super) pool_max_idle_per_host: usize,
    pub(super) tcp_keepalive: Option<Duration>,
    pub(super) tcp_nodelay: bool,
    pub(super) http2_keep_alive_interval: Option<Duration>,
    pub(super) http2_keep_alive_timeout: Duration,
    pub(super) http2_keep_alive_while_idle: bool,
}

impl UpstreamClientProfile {
    pub(super) fn from_upstream(upstream: &Upstream) -> Self {
        Self {
            tls: upstream.tls.clone(),
            dns: upstream.dns.clone(),
            tls_versions: upstream.tls_versions.clone(),
            server_verify_depth: upstream.server_verify_depth,
            server_crl_path: upstream.server_crl_path.clone(),
            client_identity: upstream.client_identity.clone(),
            protocol: upstream.protocol,
            server_name: upstream.server_name,
            server_name_override: upstream.server_name_override.clone(),
            connect_timeout: upstream.connect_timeout,
            pool_idle_timeout: upstream.pool_idle_timeout,
            pool_max_idle_per_host: upstream.pool_max_idle_per_host,
            tcp_keepalive: upstream.tcp_keepalive,
            tcp_nodelay: upstream.tcp_nodelay,
            http2_keep_alive_interval: upstream.http2_keep_alive_interval,
            http2_keep_alive_timeout: upstream.http2_keep_alive_timeout,
            http2_keep_alive_while_idle: upstream.http2_keep_alive_while_idle,
        }
    }
}
