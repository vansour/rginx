use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::tls::{ClientIdentity, TlsVersion};

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
            server_name: settings.server_name,
            server_name_override: settings.server_name_override,
            tls_versions: settings.tls_versions,
            server_verify_depth: settings.server_verify_depth,
            server_crl_path: settings.server_crl_path,
            client_identity: settings.client_identity,
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
