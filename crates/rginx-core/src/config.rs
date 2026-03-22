use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use http::{HeaderName, HeaderValue, StatusCode};
use ipnet::IpNet;

#[derive(Debug, Clone)]
pub struct ConfigSnapshot {
    pub runtime: RuntimeSettings,
    pub server: Server,
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
}

#[derive(Debug, Clone)]
pub struct VirtualHost {
    pub id: String,
    pub server_names: Vec<String>,
    pub routes: Vec<Route>,
    pub tls: Option<ServerTls>,
}

impl VirtualHost {
    pub fn matches_host(&self, host: &str) -> bool {
        if self.server_names.is_empty() {
            return true;
        }
        let hostname = host.split(':').next().unwrap_or(host).to_lowercase();
        self.server_names.iter().any(|pattern| {
            let pattern_lower = pattern.to_lowercase();
            if let Some(suffix) = pattern_lower.strip_prefix("*.") {
                hostname.ends_with(&format!(".{suffix}")) || hostname == suffix
            } else {
                hostname == pattern_lower
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeSettings {
    pub shutdown_timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct Server {
    pub listen_addr: SocketAddr,
    pub trusted_proxies: Vec<IpNet>,
    pub keep_alive: bool,
    pub max_headers: Option<usize>,
    pub max_request_body_bytes: Option<usize>,
    pub max_connections: Option<usize>,
    pub header_read_timeout: Option<Duration>,
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
    pub id: String,
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

    pub fn id_fragment(&self) -> String {
        match self {
            Self::Exact(path) => format!("exact:{path}"),
            Self::Prefix(path) => format!("prefix:{path}"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum RouteAction {
    Static(StaticResponse),
    Proxy(ProxyTarget),
    File(FileTarget),
    Return(ReturnAction),
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
    pub preserve_host: bool,
    pub strip_prefix: Option<String>,
    pub proxy_set_headers: Vec<(HeaderName, HeaderValue)>,
}

#[derive(Debug, Clone)]
pub struct FileTarget {
    pub root: PathBuf,
    pub index: Option<String>,
    pub try_files: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ReturnAction {
    pub status: StatusCode,
    pub location: String,
    pub body: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ActiveHealthCheck {
    pub path: String,
    pub interval: Duration,
    pub timeout: Duration,
    pub healthy_successes_required: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpstreamProtocol {
    Auto,
    Http1,
    Http2,
}

impl UpstreamProtocol {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Http1 => "http1",
            Self::Http2 => "http2",
        }
    }
}

#[derive(Debug, Clone)]
pub struct UpstreamSettings {
    pub protocol: UpstreamProtocol,
    pub server_name_override: Option<String>,
    pub request_timeout: Duration,
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
        settings: UpstreamSettings,
    ) -> Self {
        Self {
            name,
            peers,
            tls,
            protocol: settings.protocol,
            server_name_override: settings.server_name_override,
            request_timeout: settings.request_timeout,
            max_replayable_request_body_bytes: settings.max_replayable_request_body_bytes,
            unhealthy_after_failures: settings.unhealthy_after_failures,
            unhealthy_cooldown: settings.unhealthy_cooldown,
            active_health_check: settings.active_health_check,
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

    use super::{
        ConfigSnapshot, Route, RouteAccessControl, RouteAction, RouteMatcher, RuntimeSettings,
        Server, StaticResponse, VirtualHost,
    };
    use http::StatusCode;
    use std::collections::HashMap;
    use std::time::Duration;

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
            keep_alive: true,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout: None,
            tls: None,
        };

        assert!(server.is_trusted_proxy("10.1.2.3".parse::<IpAddr>().unwrap()));
        assert!(server.is_trusted_proxy("::1".parse::<IpAddr>().unwrap()));
        assert!(!server.is_trusted_proxy("192.0.2.10".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn config_snapshot_counts_routes_across_all_vhosts() {
        let snapshot = ConfigSnapshot {
            runtime: RuntimeSettings { shutdown_timeout: Duration::from_secs(1) },
            server: Server {
                listen_addr: "127.0.0.1:8080".parse().unwrap(),
                trusted_proxies: Vec::new(),
                keep_alive: true,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout: None,
                tls: None,
            },
            default_vhost: VirtualHost {
                id: "server".to_string(),
                server_names: Vec::new(),
                routes: vec![route("/")],
                tls: None,
            },
            vhosts: vec![
                VirtualHost {
                    id: "servers[0]".to_string(),
                    server_names: vec!["api.example.com".to_string()],
                    routes: vec![route("/users"), route("/status")],
                    tls: None,
                },
                VirtualHost {
                    id: "servers[1]".to_string(),
                    server_names: vec!["app.example.com".to_string()],
                    routes: vec![route("/")],
                    tls: None,
                },
            ],
            upstreams: HashMap::new(),
        };

        assert_eq!(snapshot.total_vhost_count(), 3);
        assert_eq!(snapshot.total_route_count(), 4);
    }

    fn route(path: &str) -> Route {
        Route {
            id: format!("test|exact:{path}"),
            matcher: RouteMatcher::Exact(path.to_string()),
            action: RouteAction::Static(StaticResponse {
                status: StatusCode::OK,
                content_type: "text/plain; charset=utf-8".to_string(),
                body: "ok\n".to_string(),
            }),
            access_control: RouteAccessControl::default(),
            rate_limit: None,
        }
    }
}
