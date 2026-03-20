use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use http::StatusCode;
use ipnet::IpNet;

#[derive(Debug, Clone)]
pub struct ConfigSnapshot {
    pub runtime: RuntimeSettings,
    pub server: Server,
    pub routes: Vec<Route>,
    pub upstreams: HashMap<String, Arc<Upstream>>,
}

#[derive(Debug, Clone)]
pub struct RuntimeSettings {
    pub shutdown_timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct Server {
    pub listen_addr: SocketAddr,
    pub trusted_proxies: Vec<IpNet>,
    pub tls: Option<ServerTls>,
}

impl Server {
    pub fn is_trusted_proxy(&self, ip: IpAddr) -> bool {
        self.trusted_proxies.iter().any(|cidr| cidr.contains(&ip))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerTls {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Route {
    pub matcher: RouteMatcher,
    pub action: RouteAction,
    pub access_control: RouteAccessControl,
    pub rate_limit: Option<RouteRateLimit>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteAccessControl {
    pub allow_cidrs: Vec<IpNet>,
    pub deny_cidrs: Vec<IpNet>,
}

impl RouteAccessControl {
    pub fn new(allow_cidrs: Vec<IpNet>, deny_cidrs: Vec<IpNet>) -> Self {
        Self { allow_cidrs, deny_cidrs }
    }

    pub fn allows(&self, ip: IpAddr) -> bool {
        if self.deny_cidrs.iter().any(|cidr| cidr.contains(&ip)) {
            return false;
        }

        self.allow_cidrs.is_empty() || self.allow_cidrs.iter().any(|cidr| cidr.contains(&ip))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteRateLimit {
    pub requests_per_sec: u32,
    pub burst: u32,
}

impl RouteRateLimit {
    pub fn new(requests_per_sec: u32, burst: u32) -> Self {
        Self { requests_per_sec, burst }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteMatcher {
    Exact(String),
    Prefix(String),
}

impl RouteMatcher {
    pub fn matches(&self, path: &str) -> bool {
        match self {
            Self::Exact(expected) => path == expected,
            Self::Prefix(prefix) if prefix == "/" => true,
            Self::Prefix(prefix) => {
                path == prefix
                    || path.strip_prefix(prefix).is_some_and(|remainder| remainder.starts_with('/'))
            }
        }
    }

    pub fn priority(&self) -> (u8, usize) {
        match self {
            Self::Exact(path) => (2, path.len()),
            Self::Prefix(path) => (1, path.len()),
        }
    }
}

#[derive(Debug, Clone)]
pub enum RouteAction {
    Static(StaticResponse),
    Proxy(ProxyTarget),
    Status,
    Metrics,
}

#[derive(Debug, Clone)]
pub struct StaticResponse {
    pub status: StatusCode,
    pub content_type: String,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct ProxyTarget {
    pub upstream_name: String,
    pub upstream: Arc<Upstream>,
}

#[derive(Debug, Clone)]
pub struct ActiveHealthCheck {
    pub path: String,
    pub interval: Duration,
    pub timeout: Duration,
    pub healthy_successes_required: u32,
}

#[derive(Debug)]
pub struct Upstream {
    pub name: String,
    pub peers: Vec<UpstreamPeer>,
    pub tls: UpstreamTls,
    pub server_name_override: Option<String>,
    pub request_timeout: Duration,
    pub max_replayable_request_body_bytes: usize,
    pub unhealthy_after_failures: u32,
    pub unhealthy_cooldown: Duration,
    pub active_health_check: Option<ActiveHealthCheck>,
    cursor: AtomicUsize,
}

impl Upstream {
    pub fn new(
        name: String,
        peers: Vec<UpstreamPeer>,
        tls: UpstreamTls,
        server_name_override: Option<String>,
        request_timeout: Duration,
        max_replayable_request_body_bytes: usize,
        unhealthy_after_failures: u32,
        unhealthy_cooldown: Duration,
        active_health_check: Option<ActiveHealthCheck>,
    ) -> Self {
        Self {
            name,
            peers,
            tls,
            server_name_override,
            request_timeout,
            max_replayable_request_body_bytes,
            unhealthy_after_failures,
            unhealthy_cooldown,
            active_health_check,
            cursor: AtomicUsize::new(0),
        }
    }

    pub fn next_peer(&self) -> Option<UpstreamPeer> {
        if self.peers.is_empty() {
            return None;
        }

        let index = self.cursor.fetch_add(1, Ordering::Relaxed) % self.peers.len();
        Some(self.peers[index].clone())
    }

    pub fn next_peers(&self, limit: usize) -> Vec<UpstreamPeer> {
        if self.peers.is_empty() || limit == 0 {
            return Vec::new();
        }

        let start = self.cursor.fetch_add(1, Ordering::Relaxed) % self.peers.len();
        let count = limit.min(self.peers.len());

        (0..count).map(|offset| self.peers[(start + offset) % self.peers.len()].clone()).collect()
    }
}

#[derive(Debug, Clone)]
pub struct UpstreamPeer {
    pub url: String,
    pub scheme: String,
    pub authority: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UpstreamTls {
    NativeRoots,
    CustomCa { ca_cert_path: PathBuf },
    Insecure,
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::{RouteAccessControl, Server};

    #[test]
    fn route_access_control_allows_when_lists_are_empty() {
        let access_control = RouteAccessControl::default();

        assert!(access_control.allows("192.0.2.10".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn route_access_control_restricts_to_allow_list() {
        let access_control = RouteAccessControl::new(
            vec!["127.0.0.1/32".parse().unwrap(), "::1/128".parse().unwrap()],
            Vec::new(),
        );

        assert!(access_control.allows("127.0.0.1".parse::<IpAddr>().unwrap()));
        assert!(!access_control.allows("192.0.2.10".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn route_access_control_denies_before_allowing() {
        let access_control = RouteAccessControl::new(
            vec!["10.0.0.0/8".parse().unwrap()],
            vec!["10.0.0.5/32".parse().unwrap()],
        );

        assert!(access_control.allows("10.1.2.3".parse::<IpAddr>().unwrap()));
        assert!(!access_control.allows("10.0.0.5".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn server_matches_trusted_proxy_cidrs() {
        let server = Server {
            listen_addr: "127.0.0.1:8080".parse().unwrap(),
            trusted_proxies: vec!["10.0.0.0/8".parse().unwrap(), "::1/128".parse().unwrap()],
            tls: None,
        };

        assert!(server.is_trusted_proxy("10.1.2.3".parse::<IpAddr>().unwrap()));
        assert!(server.is_trusted_proxy("::1".parse::<IpAddr>().unwrap()));
        assert!(!server.is_trusted_proxy("192.0.2.10".parse::<IpAddr>().unwrap()));
    }
}
