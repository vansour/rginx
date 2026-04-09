use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
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
    RouteMatcher, RouteRateLimit,
};
pub use tls::{
    ClientIdentity, ServerCertificateBundle, ServerClientAuthMode, ServerClientAuthPolicy,
    ServerTls, TlsCipherSuite, TlsKeyExchangeGroup, TlsVersion, VirtualHostTls,
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

#[derive(Debug, Clone)]
pub struct ConfigSnapshot {
    pub runtime: RuntimeSettings,
    pub server: Server,
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

    pub fn tls_enabled(&self) -> bool {
        self.listeners.iter().any(Listener::tls_enabled)
    }

    pub fn listener(&self, id: &str) -> Option<&Listener> {
        self.listeners.iter().find(|listener| listener.id == id)
    }
}

#[derive(Debug, Clone)]
pub struct Listener {
    pub id: String,
    pub name: String,
    pub server: Server,
    pub tls_termination_enabled: bool,
    pub proxy_protocol_enabled: bool,
}

impl Listener {
    pub fn tls_enabled(&self) -> bool {
        self.tls_termination_enabled
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
        if self.server_names.is_empty() {
            return true;
        }
        self.server_names.iter().any(|pattern| match_server_name(pattern, host).is_some())
    }
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
