use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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

#[derive(Debug, Clone)]
pub struct UpstreamSettings {
    pub protocol: UpstreamProtocol,
    pub load_balance: UpstreamLoadBalance,
    pub server_name_override: Option<String>,
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
    pub server_name_override: Option<String>,
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
            load_balance: settings.load_balance,
            server_name_override: settings.server_name_override,
            request_timeout: settings.request_timeout,
            connect_timeout: settings.connect_timeout,
            write_timeout: settings.write_timeout,
            idle_timeout: settings.idle_timeout,
            pool_idle_timeout: settings.pool_idle_timeout,
            pool_max_idle_per_host: settings.pool_max_idle_per_host,
            tcp_keepalive: settings.tcp_keepalive,
            tcp_nodelay: settings.tcp_nodelay,
            http2_keep_alive_interval: settings.http2_keep_alive_interval,
            http2_keep_alive_timeout: settings.http2_keep_alive_timeout,
            http2_keep_alive_while_idle: settings.http2_keep_alive_while_idle,
            max_replayable_request_body_bytes: settings.max_replayable_request_body_bytes,
            unhealthy_after_failures: settings.unhealthy_after_failures,
            unhealthy_cooldown: settings.unhealthy_cooldown,
            active_health_check: settings.active_health_check,
            cursor: AtomicUsize::new(0),
        }
    }

    pub fn next_peer(&self) -> Option<UpstreamPeer> {
        self.next_peers(1).into_iter().next()
    }

    pub fn next_peers(&self, limit: usize) -> Vec<UpstreamPeer> {
        let primary = self.next_peers_in_pool(limit, false);
        if primary.is_empty() { self.next_peers_in_pool(limit, true) } else { primary }
    }

    pub fn primary_next_peers(&self, limit: usize) -> Vec<UpstreamPeer> {
        self.next_peers_in_pool(limit, false)
    }

    pub fn backup_next_peers(&self, limit: usize) -> Vec<UpstreamPeer> {
        self.next_peers_in_pool(limit, true)
    }

    pub fn has_primary_peers(&self) -> bool {
        self.peers.iter().any(|peer| !peer.backup)
    }

    pub fn peers_for_client_ip(&self, client_ip: IpAddr, limit: usize) -> Vec<UpstreamPeer> {
        let primary = self.peers_for_client_ip_in_pool(client_ip, limit, false);
        if primary.is_empty() {
            self.peers_for_client_ip_in_pool(client_ip, limit, true)
        } else {
            primary
        }
    }

    pub fn primary_peers_for_client_ip(
        &self,
        client_ip: IpAddr,
        limit: usize,
    ) -> Vec<UpstreamPeer> {
        self.peers_for_client_ip_in_pool(client_ip, limit, false)
    }

    pub fn backup_peers_for_client_ip(&self, client_ip: IpAddr, limit: usize) -> Vec<UpstreamPeer> {
        self.peers_for_client_ip_in_pool(client_ip, limit, true)
    }

    fn next_peers_in_pool(&self, limit: usize, backup: bool) -> Vec<UpstreamPeer> {
        let peer_indices = self.peer_indices_for_pool(backup);
        if peer_indices.is_empty() || limit == 0 {
            return Vec::new();
        }

        let total_weight = self.total_weight_for_indices(&peer_indices);
        if total_weight == 0 {
            return peer_indices
                .iter()
                .take(limit)
                .map(|index| self.peers[*index].clone())
                .collect();
        }

        let start = self.cursor.fetch_add(1, Ordering::Relaxed) % total_weight;
        self.weighted_peers_from_indices(&peer_indices, start, limit)
    }

    fn peers_for_client_ip_in_pool(
        &self,
        client_ip: IpAddr,
        limit: usize,
        backup: bool,
    ) -> Vec<UpstreamPeer> {
        let peer_indices = self.peer_indices_for_pool(backup);
        if peer_indices.is_empty() || limit == 0 {
            return Vec::new();
        }

        match self.load_balance {
            UpstreamLoadBalance::RoundRobin => self.next_peers_in_pool(limit, backup),
            UpstreamLoadBalance::IpHash => {
                self.ip_hash_peers_in_pool(client_ip, limit, &peer_indices)
            }
            UpstreamLoadBalance::LeastConn => self.next_peers_in_pool(limit, backup),
        }
    }

    fn ip_hash_peers_in_pool(
        &self,
        client_ip: IpAddr,
        limit: usize,
        peer_indices: &[usize],
    ) -> Vec<UpstreamPeer> {
        if peer_indices.is_empty() || limit == 0 {
            return Vec::new();
        }

        let total_weight = self.total_weight_for_indices(peer_indices);
        if total_weight == 0 {
            return peer_indices
                .iter()
                .take(limit)
                .map(|index| self.peers[*index].clone())
                .collect();
        }

        let start = stable_ip_hash(client_ip) as usize % total_weight;
        self.weighted_peers_from_indices(peer_indices, start, limit)
    }

    fn peer_indices_for_pool(&self, backup: bool) -> Vec<usize> {
        self.peers
            .iter()
            .enumerate()
            .filter_map(|(index, peer)| (peer.backup == backup).then_some(index))
            .collect()
    }

    fn total_weight_for_indices(&self, peer_indices: &[usize]) -> usize {
        peer_indices.iter().map(|index| self.peers[*index].weight as usize).sum()
    }

    fn weighted_peers_from_indices(
        &self,
        peer_indices: &[usize],
        start: usize,
        limit: usize,
    ) -> Vec<UpstreamPeer> {
        let total_weight = self.total_weight_for_indices(peer_indices);
        if total_weight == 0 {
            return Vec::new();
        }

        let count = limit.min(peer_indices.len());
        let mut selected = Vec::with_capacity(count);
        let mut seen = vec![false; peer_indices.len()];

        for offset in 0..total_weight {
            let slot = (start + offset) % total_weight;
            let Some(position) = self.peer_position_for_weighted_slot(peer_indices, slot) else {
                continue;
            };

            if seen[position] {
                continue;
            }

            seen[position] = true;
            selected.push(self.peers[peer_indices[position]].clone());
            if selected.len() == count {
                break;
            }
        }

        selected
    }

    fn peer_position_for_weighted_slot(
        &self,
        peer_indices: &[usize],
        slot: usize,
    ) -> Option<usize> {
        let mut offset = 0usize;

        for (position, index) in peer_indices.iter().enumerate() {
            offset += self.peers[*index].weight as usize;
            if slot < offset {
                return Some(position);
            }
        }

        None
    }
}

fn stable_ip_hash(client_ip: IpAddr) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let octets = match client_ip {
        IpAddr::V4(addr) => addr.octets().to_vec(),
        IpAddr::V6(addr) => addr.octets().to_vec(),
    };

    octets
        .into_iter()
        .fold(FNV_OFFSET, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME))
}

#[derive(Debug, Clone)]
pub struct UpstreamPeer {
    pub url: String,
    pub scheme: String,
    pub authority: String,
    pub weight: u32,
    pub backup: bool,
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
