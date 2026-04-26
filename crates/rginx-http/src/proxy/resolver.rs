use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Instant;

use hickory_resolver::TokioResolver;
use rginx_core::UpstreamDnsPolicy;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

mod endpoint;
mod runtime;

#[derive(Debug, Clone)]
pub(crate) struct ResolvedUpstreamPeer {
    #[cfg_attr(not(test), allow(dead_code))]
    pub url: String,
    pub logical_peer_url: String,
    pub endpoint_key: String,
    pub display_url: String,
    pub scheme: String,
    pub upstream_authority: String,
    pub dial_authority: String,
    pub socket_addr: SocketAddr,
    pub server_name: String,
    pub weight: u32,
    pub backup: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UpstreamResolverCacheEntrySnapshot {
    pub hostname: String,
    pub addresses: Vec<String>,
    pub negative: bool,
    pub valid_for_ms: Option<u64>,
    pub stale_for_ms: Option<u64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UpstreamResolverRuntimeSnapshot {
    pub resolve_requests_total: u64,
    pub cache_hits_total: u64,
    pub cache_misses_total: u64,
    pub refreshes_total: u64,
    pub resolve_errors_total: u64,
    pub stale_answers_total: u64,
    pub cache_entries: Vec<UpstreamResolverCacheEntrySnapshot>,
}

#[derive(Debug, Clone)]
pub(super) struct CacheEntry {
    pub(super) addresses: Vec<IpAddr>,
    pub(super) valid_until: Instant,
    pub(super) stale_until: Instant,
    pub(super) negative: bool,
    pub(super) last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct PeerAddressing {
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) scheme: String,
    pub(super) authority: String,
    pub(super) logical_peer_url: String,
    pub(super) weight: u32,
    pub(super) backup: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct UpstreamResolver {
    pub(super) policy: UpstreamDnsPolicy,
    pub(super) resolver: TokioResolver,
    pub(super) cache: Arc<Mutex<HashMap<String, CacheEntry>>>,
    pub(super) resolve_requests_total: Arc<AtomicU64>,
    pub(super) cache_hits_total: Arc<AtomicU64>,
    pub(super) cache_misses_total: Arc<AtomicU64>,
    pub(super) refreshes_total: Arc<AtomicU64>,
    pub(super) resolve_errors_total: Arc<AtomicU64>,
    pub(super) stale_answers_total: Arc<AtomicU64>,
}
