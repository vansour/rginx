use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ipnet::IpNet;

mod access_log;
mod route;
mod tls;
mod upstream;

pub use access_log::{AccessLogFormat, AccessLogValues};
pub use route::{
    GrpcRouteMatch, ProxyTarget, ReturnAction, Route, RouteAccessControl, RouteAction,
    RouteBufferingPolicy, RouteCompressionPolicy, RouteMatcher, RouteRateLimit,
};
pub use tls::{
    ClientIdentity, OcspConfig, OcspNonceMode, OcspResponderPolicy, ServerCertificateBundle,
    ServerClientAuthMode, ServerClientAuthPolicy, ServerTls, TlsCipherSuite, TlsKeyExchangeGroup,
    TlsVersion, VirtualHostTls,
};
pub use upstream::{
    ActiveHealthCheck, Upstream, UpstreamLoadBalance, UpstreamPeer, UpstreamProtocol,
    UpstreamSettings, UpstreamTls,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ServerNameMatch {
    Exact,
    Wildcard { suffix_len: usize },
}

impl ServerNameMatch {
    pub fn priority(self) -> (u8, usize) {
        match self {
            Self::Exact => (2, 0),
            Self::Wildcard { suffix_len } => (1, suffix_len),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfigSnapshot {
    pub runtime: RuntimeSettings,
    pub listeners: Vec<Listener>,
    pub default_vhost: VirtualHost,
    pub vhosts: Vec<VirtualHost>,
    pub upstreams: HashMap<String, Arc<Upstream>>,
}

impl ConfigSnapshot {
    pub fn total_route_count(&self) -> usize {
        self.default_vhost.routes.len()
            + self.vhosts.iter().map(|vhost| vhost.routes.len()).sum::<usize>()
    }

    pub fn total_vhost_count(&self) -> usize {
        1 + self.vhosts.len()
    }

    pub fn total_listener_count(&self) -> usize {
        self.listeners.len()
    }

    pub fn total_listener_binding_count(&self) -> usize {
        self.listeners.iter().map(Listener::binding_count).sum()
    }

    pub fn tls_enabled(&self) -> bool {
        self.listeners.iter().any(Listener::tls_enabled)
    }

    pub fn http3_enabled(&self) -> bool {
        self.listeners.iter().any(Listener::http3_enabled)
    }

    pub fn listener(&self, id: &str) -> Option<&Listener> {
        self.listeners.iter().find(|listener| listener.id == id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListenerTransportKind {
    Tcp,
    Udp,
}

impl ListenerTransportKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListenerApplicationProtocol {
    Http1,
    Http2,
    Http3,
}

impl ListenerApplicationProtocol {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http1 => "http1",
            Self::Http2 => "http2",
            Self::Http3 => "http3",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ListenerHttp3 {
    pub listen_addr: SocketAddr,
    pub advertise_alt_svc: bool,
    pub alt_svc_max_age: Duration,
    pub max_concurrent_streams: usize,
    pub stream_buffer_size: usize,
    pub active_connection_id_limit: u32,
    pub retry: bool,
    pub host_key_path: Option<PathBuf>,
    pub gso: bool,
    pub early_data_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct ListenerTransportBinding {
    pub name: &'static str,
    pub kind: ListenerTransportKind,
    pub listen_addr: SocketAddr,
    pub protocols: Vec<ListenerApplicationProtocol>,
    pub advertise_alt_svc: bool,
    pub alt_svc_max_age: Option<Duration>,
    pub http3_max_concurrent_streams: Option<usize>,
    pub http3_stream_buffer_size: Option<usize>,
    pub http3_active_connection_id_limit: Option<u32>,
    pub http3_retry: Option<bool>,
    pub http3_host_key_path: Option<PathBuf>,
    pub http3_gso: Option<bool>,
    pub http3_early_data_enabled: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct Listener {
    pub id: String,
    pub name: String,
    pub server: Server,
    pub tls_termination_enabled: bool,
    pub proxy_protocol_enabled: bool,
    pub http3: Option<ListenerHttp3>,
}

impl Listener {
    pub fn tls_enabled(&self) -> bool {
        self.tls_termination_enabled
    }

    pub fn http3_enabled(&self) -> bool {
        self.http3.is_some()
    }

    pub fn binding_count(&self) -> usize {
        1 + usize::from(self.http3.is_some())
    }

    pub fn transport_bindings(&self) -> Vec<ListenerTransportBinding> {
        let mut bindings = vec![ListenerTransportBinding {
            name: "tcp",
            kind: ListenerTransportKind::Tcp,
            listen_addr: self.server.listen_addr,
            protocols: if self.tls_enabled() {
                vec![ListenerApplicationProtocol::Http1, ListenerApplicationProtocol::Http2]
            } else {
                vec![ListenerApplicationProtocol::Http1]
            },
            advertise_alt_svc: false,
            alt_svc_max_age: None,
            http3_max_concurrent_streams: None,
            http3_stream_buffer_size: None,
            http3_active_connection_id_limit: None,
            http3_retry: None,
            http3_host_key_path: None,
            http3_gso: None,
            http3_early_data_enabled: None,
        }];

        if let Some(http3) = &self.http3 {
            bindings.push(ListenerTransportBinding {
                name: "udp",
                kind: ListenerTransportKind::Udp,
                listen_addr: http3.listen_addr,
                protocols: vec![ListenerApplicationProtocol::Http3],
                advertise_alt_svc: http3.advertise_alt_svc,
                alt_svc_max_age: Some(http3.alt_svc_max_age),
                http3_max_concurrent_streams: Some(http3.max_concurrent_streams),
                http3_stream_buffer_size: Some(http3.stream_buffer_size),
                http3_active_connection_id_limit: Some(http3.active_connection_id_limit),
                http3_retry: Some(http3.retry),
                http3_host_key_path: http3.host_key_path.clone(),
                http3_gso: Some(http3.gso),
                http3_early_data_enabled: Some(http3.early_data_enabled),
            });
        }

        bindings
    }
}

#[derive(Debug, Clone)]
pub struct VirtualHost {
    pub id: String,
    pub server_names: Vec<String>,
    pub routes: Vec<Route>,
    pub tls: Option<VirtualHostTls>,
}

impl VirtualHost {
    pub fn matches_host(&self, host: &str) -> bool {
        self.server_names.is_empty() || self.best_server_name_match(host).is_some()
    }

    pub fn best_server_name_match(&self, host: &str) -> Option<ServerNameMatch> {
        best_matching_server_name_pattern(self.server_names.iter().map(String::as_str), host)
            .map(|(_, matched)| matched)
    }
}

pub fn best_matching_server_name_pattern<'a>(
    patterns: impl IntoIterator<Item = &'a str>,
    host: &str,
) -> Option<(&'a str, ServerNameMatch)> {
    patterns
        .into_iter()
        .filter_map(|pattern| match_server_name(pattern, host).map(|matched| (pattern, matched)))
        .max_by(|left, right| {
            left.1.priority().cmp(&right.1.priority()).then_with(|| right.0.cmp(left.0))
        })
}

pub fn match_server_name(pattern: &str, host: &str) -> Option<ServerNameMatch> {
    let hostname = normalize_host_for_match(host);
    let pattern = pattern.trim().to_ascii_lowercase();

    if pattern.is_empty() {
        return None;
    }

    if let Some(suffix) = pattern.strip_prefix("*.") {
        if suffix.is_empty() || hostname == suffix {
            return None;
        }

        return hostname
            .ends_with(&format!(".{suffix}"))
            .then_some(ServerNameMatch::Wildcard { suffix_len: suffix.len() });
    }

    (hostname == pattern).then_some(ServerNameMatch::Exact)
}

fn normalize_host_for_match(host: &str) -> String {
    if let Some(rest) = host.strip_prefix('[')
        && let Some((addr, _)) = rest.split_once(']')
    {
        return addr.to_ascii_lowercase();
    }

    host.split(':').next().unwrap_or(host).to_ascii_lowercase()
}

#[derive(Debug, Clone)]
pub struct RuntimeSettings {
    pub shutdown_timeout: Duration,
    pub worker_threads: Option<usize>,
    pub accept_workers: usize,
}

#[derive(Debug, Clone)]
pub struct Server {
    pub listen_addr: SocketAddr,
    pub default_certificate: Option<String>,
    pub trusted_proxies: Vec<IpNet>,
    pub keep_alive: bool,
    pub max_headers: Option<usize>,
    pub max_request_body_bytes: Option<usize>,
    pub max_connections: Option<usize>,
    pub header_read_timeout: Option<Duration>,
    pub request_body_read_timeout: Option<Duration>,
    pub response_write_timeout: Option<Duration>,
    pub access_log_format: Option<AccessLogFormat>,
    pub tls: Option<ServerTls>,
}

impl Server {
    pub fn is_trusted_proxy(&self, ip: IpAddr) -> bool {
        self.trusted_proxies.iter().any(|cidr| cidr.contains(&ip))
    }
}

#[cfg(test)]
mod tests;
