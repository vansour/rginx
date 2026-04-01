use std::collections::HashMap;
use std::fmt::Write as _;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use http::{HeaderName, HeaderValue, StatusCode};
use ipnet::IpNet;

use crate::{Error, Result};

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

    pub fn tls_enabled(&self) -> bool {
        self.default_vhost.tls.is_some() || self.vhosts.iter().any(|vhost| vhost.tls.is_some())
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
    pub worker_threads: Option<usize>,
    pub accept_workers: usize,
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
    pub request_body_read_timeout: Option<Duration>,
    pub response_write_timeout: Option<Duration>,
    pub access_log_format: Option<AccessLogFormat>,
    pub config_api_token: Option<String>,
    pub tls: Option<ServerTls>,
}

impl Server {
    pub fn is_trusted_proxy(&self, ip: IpAddr) -> bool {
        self.trusted_proxies.iter().any(|cidr| cidr.contains(&ip))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessLogFormat {
    template: String,
    segments: Vec<AccessLogSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AccessLogSegment {
    Literal(String),
    Variable(AccessLogVariable),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccessLogVariable {
    RequestId,
    RemoteAddr,
    PeerAddr,
    Method,
    Host,
    Path,
    Request,
    Status,
    BodyBytesSent,
    ElapsedMs,
    ClientIpSource,
    Vhost,
    Route,
    Scheme,
    HttpVersion,
    UserAgent,
    Referer,
    GrpcProtocol,
    GrpcService,
    GrpcMethod,
    GrpcStatus,
    GrpcMessage,
}

#[derive(Debug, Clone, Copy)]
pub struct AccessLogValues<'a> {
    pub request_id: &'a str,
    pub remote_addr: &'a str,
    pub peer_addr: &'a str,
    pub method: &'a str,
    pub host: &'a str,
    pub path: &'a str,
    pub request: &'a str,
    pub status: u16,
    pub body_bytes_sent: Option<u64>,
    pub elapsed_ms: u64,
    pub client_ip_source: &'a str,
    pub vhost: &'a str,
    pub route: &'a str,
    pub scheme: &'a str,
    pub http_version: &'a str,
    pub user_agent: Option<&'a str>,
    pub referer: Option<&'a str>,
    pub grpc_protocol: Option<&'a str>,
    pub grpc_service: Option<&'a str>,
    pub grpc_method: Option<&'a str>,
    pub grpc_status: Option<&'a str>,
    pub grpc_message: Option<&'a str>,
}

impl AccessLogFormat {
    pub fn parse(template: impl Into<String>) -> Result<Self> {
        let template = template.into();
        let mut segments = Vec::new();
        let bytes = template.as_bytes();
        let mut literal_start = 0usize;
        let mut index = 0usize;

        while index < bytes.len() {
            if bytes[index] != b'$' {
                index += 1;
                continue;
            }

            if literal_start < index {
                segments
                    .push(AccessLogSegment::Literal(template[literal_start..index].to_string()));
            }

            if let Some(next) = bytes.get(index + 1) {
                if *next == b'$' {
                    segments.push(AccessLogSegment::Literal("$".to_string()));
                    index += 2;
                    literal_start = index;
                    continue;
                }

                if *next == b'{' {
                    let Some(relative_end) = template[index + 2..].find('}') else {
                        return Err(Error::Config(
                            "access_log_format contains an unterminated `${...}` variable"
                                .to_string(),
                        ));
                    };
                    let end = index + 2 + relative_end;
                    let name = &template[index + 2..end];
                    segments.push(AccessLogSegment::Variable(parse_access_log_variable(name)?));
                    index = end + 1;
                    literal_start = index;
                    continue;
                }
            }

            let mut end = index + 1;
            while end < bytes.len() && is_access_log_variable_char(bytes[end]) {
                end += 1;
            }

            if end == index + 1 {
                segments.push(AccessLogSegment::Literal("$".to_string()));
                index += 1;
                literal_start = index;
                continue;
            }

            let name = &template[index + 1..end];
            segments.push(AccessLogSegment::Variable(parse_access_log_variable(name)?));
            index = end;
            literal_start = end;
        }

        if literal_start < template.len() {
            segments.push(AccessLogSegment::Literal(template[literal_start..].to_string()));
        }

        Ok(Self { template, segments })
    }

    pub fn template(&self) -> &str {
        &self.template
    }

    pub fn render(&self, values: &AccessLogValues<'_>) -> String {
        let mut rendered = String::with_capacity(self.template.len() + 64);

        for segment in &self.segments {
            match segment {
                AccessLogSegment::Literal(literal) => rendered.push_str(literal),
                AccessLogSegment::Variable(variable) => match variable {
                    AccessLogVariable::RequestId => rendered.push_str(values.request_id),
                    AccessLogVariable::RemoteAddr => rendered.push_str(values.remote_addr),
                    AccessLogVariable::PeerAddr => rendered.push_str(values.peer_addr),
                    AccessLogVariable::Method => rendered.push_str(values.method),
                    AccessLogVariable::Host => {
                        rendered.push_str(fallback_access_log_value(values.host))
                    }
                    AccessLogVariable::Path => rendered.push_str(values.path),
                    AccessLogVariable::Request => rendered.push_str(values.request),
                    AccessLogVariable::Status => {
                        let _ = write!(rendered, "{}", values.status);
                    }
                    AccessLogVariable::BodyBytesSent => {
                        if let Some(bytes) = values.body_bytes_sent {
                            let _ = write!(rendered, "{bytes}");
                        } else {
                            rendered.push('-');
                        }
                    }
                    AccessLogVariable::ElapsedMs => {
                        let _ = write!(rendered, "{}", values.elapsed_ms);
                    }
                    AccessLogVariable::ClientIpSource => rendered.push_str(values.client_ip_source),
                    AccessLogVariable::Vhost => rendered.push_str(values.vhost),
                    AccessLogVariable::Route => rendered.push_str(values.route),
                    AccessLogVariable::Scheme => rendered.push_str(values.scheme),
                    AccessLogVariable::HttpVersion => rendered.push_str(values.http_version),
                    AccessLogVariable::UserAgent => {
                        rendered.push_str(fallback_access_log_option(values.user_agent))
                    }
                    AccessLogVariable::Referer => {
                        rendered.push_str(fallback_access_log_option(values.referer))
                    }
                    AccessLogVariable::GrpcProtocol => {
                        rendered.push_str(fallback_access_log_option(values.grpc_protocol))
                    }
                    AccessLogVariable::GrpcService => {
                        rendered.push_str(fallback_access_log_option(values.grpc_service))
                    }
                    AccessLogVariable::GrpcMethod => {
                        rendered.push_str(fallback_access_log_option(values.grpc_method))
                    }
                    AccessLogVariable::GrpcStatus => {
                        rendered.push_str(fallback_access_log_option(values.grpc_status))
                    }
                    AccessLogVariable::GrpcMessage => {
                        rendered.push_str(fallback_access_log_option(values.grpc_message))
                    }
                },
            }
        }

        rendered
    }
}

fn parse_access_log_variable(name: &str) -> Result<AccessLogVariable> {
    match name {
        "request_id" => Ok(AccessLogVariable::RequestId),
        "remote_addr" | "client_ip" => Ok(AccessLogVariable::RemoteAddr),
        "peer_addr" => Ok(AccessLogVariable::PeerAddr),
        "method" | "request_method" => Ok(AccessLogVariable::Method),
        "host" => Ok(AccessLogVariable::Host),
        "path" | "request_uri" => Ok(AccessLogVariable::Path),
        "request" => Ok(AccessLogVariable::Request),
        "status" => Ok(AccessLogVariable::Status),
        "body_bytes_sent" | "bytes_sent" => Ok(AccessLogVariable::BodyBytesSent),
        "request_time_ms" | "elapsed_ms" => Ok(AccessLogVariable::ElapsedMs),
        "client_ip_source" => Ok(AccessLogVariable::ClientIpSource),
        "vhost" | "server_name" => Ok(AccessLogVariable::Vhost),
        "route" => Ok(AccessLogVariable::Route),
        "scheme" => Ok(AccessLogVariable::Scheme),
        "http_version" | "server_protocol" => Ok(AccessLogVariable::HttpVersion),
        "http_user_agent" | "user_agent" => Ok(AccessLogVariable::UserAgent),
        "http_referer" | "referer" => Ok(AccessLogVariable::Referer),
        "grpc_protocol" => Ok(AccessLogVariable::GrpcProtocol),
        "grpc_service" => Ok(AccessLogVariable::GrpcService),
        "grpc_method" => Ok(AccessLogVariable::GrpcMethod),
        "grpc_status" => Ok(AccessLogVariable::GrpcStatus),
        "grpc_message" => Ok(AccessLogVariable::GrpcMessage),
        _ => Err(Error::Config(format!("access_log_format variable `${name}` is not supported"))),
    }
}

fn is_access_log_variable_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn fallback_access_log_value(value: &str) -> &str {
    if value.is_empty() { "-" } else { value }
}

fn fallback_access_log_option(value: Option<&str>) -> &str {
    value.filter(|value| !value.is_empty()).unwrap_or("-")
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
    pub grpc_match: Option<GrpcRouteMatch>,
    pub action: RouteAction,
    pub access_control: RouteAccessControl,
    pub rate_limit: Option<RouteRateLimit>,
}

impl Route {
    pub fn priority(&self) -> (u8, usize, u8) {
        let (matcher_rank, matcher_len) = self.matcher.priority();
        let grpc_rank = self.grpc_match.as_ref().map_or(0, |grpc_match| grpc_match.priority());
        (matcher_rank, matcher_len, grpc_rank)
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrpcRouteMatch {
    pub service: Option<String>,
    pub method: Option<String>,
}

impl GrpcRouteMatch {
    pub fn matches(&self, service: &str, method: &str) -> bool {
        self.service.as_deref().is_none_or(|expected| expected == service)
            && self.method.as_deref().is_none_or(|expected| expected == method)
    }

    pub fn priority(&self) -> u8 {
        u8::from(self.service.is_some()) + u8::from(self.method.is_some())
    }

    pub fn id_fragment(&self) -> String {
        let mut fragments = Vec::new();
        if let Some(service) = &self.service {
            fragments.push(format!("service={service}"));
        }
        if let Some(method) = &self.method {
            fragments.push(format!("method={method}"));
        }
        format!("grpc:{}", fragments.join(","))
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
    Config,
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
    pub autoindex: bool,
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
        AccessLogFormat, AccessLogValues, ConfigSnapshot, Route, RouteAccessControl, RouteAction,
        RouteMatcher, RuntimeSettings, Server, StaticResponse, VirtualHost,
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
            request_body_read_timeout: None,
            response_write_timeout: None,
            access_log_format: None,
            tls: None,
        };

        assert!(server.is_trusted_proxy("10.1.2.3".parse::<IpAddr>().unwrap()));
        assert!(server.is_trusted_proxy("::1".parse::<IpAddr>().unwrap()));
        assert!(!server.is_trusted_proxy("192.0.2.10".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn config_snapshot_counts_routes_across_all_vhosts() {
        let snapshot = ConfigSnapshot {
            runtime: RuntimeSettings {
                shutdown_timeout: Duration::from_secs(1),
                worker_threads: None,
                accept_workers: 1,
            },
            server: Server {
                listen_addr: "127.0.0.1:8080".parse().unwrap(),
                trusted_proxies: Vec::new(),
                keep_alive: true,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout: None,
                request_body_read_timeout: None,
                response_write_timeout: None,
                access_log_format: None,
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

    #[test]
    fn access_log_format_renders_nginx_style_variables() {
        let format = AccessLogFormat::parse(
            "reqid=$request_id remote=$remote_addr request=\"$request\" status=$status bytes=$body_bytes_sent elapsed=$request_time_ms ua=\"$http_user_agent\" referer=\"$http_referer\" grpc=$grpc_protocol svc=$grpc_service rpc=$grpc_method grpc_status=$grpc_status grpc_message=\"$grpc_message\"",
        )
        .expect("access log format should parse");

        let rendered = format.render(&AccessLogValues {
            request_id: "rginx-0000000000000042",
            remote_addr: "203.0.113.10",
            peer_addr: "10.0.0.5:45678",
            method: "GET",
            host: "app.example.com",
            path: "/hello?name=rginx",
            request: "GET /hello?name=rginx HTTP/1.1",
            status: 200,
            body_bytes_sent: Some(12),
            elapsed_ms: 7,
            client_ip_source: "x_forwarded_for",
            vhost: "servers[0]",
            route: "servers[0]/routes[0]|exact:/hello",
            scheme: "https",
            http_version: "HTTP/1.1",
            user_agent: Some("curl/8.7.1"),
            referer: None,
            grpc_protocol: Some("grpc-web"),
            grpc_service: Some("grpc.health.v1.Health"),
            grpc_method: Some("Check"),
            grpc_status: Some("0"),
            grpc_message: Some("ok"),
        });

        assert_eq!(
            rendered,
            "reqid=rginx-0000000000000042 remote=203.0.113.10 request=\"GET /hello?name=rginx HTTP/1.1\" status=200 bytes=12 elapsed=7 ua=\"curl/8.7.1\" referer=\"-\" grpc=grpc-web svc=grpc.health.v1.Health rpc=Check grpc_status=0 grpc_message=\"ok\""
        );
    }

    #[test]
    fn access_log_format_rejects_unknown_variables() {
        let error = AccessLogFormat::parse("status=$status trace=$trace_id")
            .expect_err("unknown variable should fail");
        assert!(
            error.to_string().contains("access_log_format variable `$trace_id` is not supported")
        );
    }

    #[test]
    fn access_log_format_supports_braced_variables_and_literal_dollar() {
        let format = AccessLogFormat::parse("$$ ${request_id} ${status}")
            .expect("access log format should parse");

        let rendered = format.render(&AccessLogValues {
            request_id: "req-1",
            remote_addr: "127.0.0.1",
            peer_addr: "127.0.0.1:80",
            method: "GET",
            host: "",
            path: "/",
            request: "GET / HTTP/1.1",
            status: 204,
            body_bytes_sent: None,
            elapsed_ms: 1,
            client_ip_source: "peer",
            vhost: "server",
            route: "server/routes[0]|exact:/",
            scheme: "http",
            http_version: "HTTP/1.1",
            user_agent: None,
            referer: None,
            grpc_protocol: None,
            grpc_service: None,
            grpc_method: None,
            grpc_status: None,
            grpc_message: None,
        });

        assert_eq!(rendered, "$ req-1 204");
    }

    fn route(path: &str) -> Route {
        Route {
            id: format!("test|exact:{path}"),
            matcher: RouteMatcher::Exact(path.to_string()),
            grpc_match: None,
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
