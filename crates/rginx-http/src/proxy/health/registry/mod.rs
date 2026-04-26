//! Per-upstream passive and active health state used by peer selection.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rginx_core::{ConfigSnapshot, Upstream, UpstreamLoadBalance, UpstreamPeer};
use serde::{Deserialize, Serialize};

use crate::proxy::clients::ProxyClient;
use crate::proxy::{HealthChangeNotifier, ResolvedUpstreamPeer, UpstreamResolverRuntimeSnapshot};

mod guards;
mod policy;
mod selection;
mod snapshot;
mod state;
#[cfg(test)]
mod tests;

pub(crate) use guards::{ActivePeerBody, ActivePeerGuard};

#[derive(Debug, Clone, Copy)]
struct PeerHealthPolicy {
    unhealthy_after_failures: u32,
    cooldown: Duration,
    active_health_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PeerFailureStatus {
    pub consecutive_failures: u32,
    pub entered_cooldown: bool,
}

#[derive(Debug, Default)]
struct PassiveHealthState {
    consecutive_failures: u32,
    unhealthy_until: Option<Instant>,
    pending_recovery: bool,
}

#[derive(Debug, Default)]
struct ActiveHealthState {
    unhealthy: bool,
    consecutive_successes: u32,
}

#[derive(Debug, Default)]
struct PeerHealthState {
    passive: PassiveHealthState,
    active: ActiveHealthState,
    active_requests: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActiveProbeStatus {
    pub healthy: bool,
    pub recovered: bool,
    pub consecutive_successes: u32,
}

#[derive(Debug, Default)]
struct PeerHealth {
    state: Mutex<PeerHealthState>,
}

type PeerHealthMap = HashMap<String, Arc<PeerHealth>>;
type UpstreamPeerHealthMap = HashMap<String, PeerHealthMap>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerHealthSnapshot {
    pub peer_url: String,
    pub backup: bool,
    pub weight: u32,
    pub available: bool,
    pub passive_consecutive_failures: u32,
    pub passive_cooldown_remaining_ms: Option<u64>,
    pub passive_pending_recovery: bool,
    pub active_unhealthy: bool,
    pub active_consecutive_successes: u32,
    pub active_requests: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedEndpointHealthSnapshot {
    pub endpoint_key: String,
    pub logical_peer_url: String,
    pub display_url: String,
    pub dial_addr: String,
    pub server_name: String,
    pub backup: bool,
    pub weight: u32,
    pub available: bool,
    pub passive_consecutive_failures: u32,
    pub passive_cooldown_remaining_ms: Option<u64>,
    pub passive_pending_recovery: bool,
    pub active_unhealthy: bool,
    pub active_consecutive_successes: u32,
    pub active_requests: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamHealthSnapshot {
    pub upstream_name: String,
    pub unhealthy_after_failures: u32,
    pub cooldown_ms: u64,
    pub active_health_enabled: bool,
    pub resolver: UpstreamResolverRuntimeSnapshot,
    pub peers: Vec<PeerHealthSnapshot>,
    pub endpoints: Vec<ResolvedEndpointHealthSnapshot>,
}

#[derive(Clone)]
pub(crate) struct PeerHealthRegistry {
    policies: Arc<HashMap<String, PeerHealthPolicy>>,
    peers: Arc<UpstreamPeerHealthMap>,
    endpoint_peers: Arc<Mutex<UpstreamPeerHealthMap>>,
    notifier: Option<HealthChangeNotifier>,
}

pub(crate) struct SelectedPeers {
    pub peers: Vec<ResolvedUpstreamPeer>,
    pub skipped_unhealthy: usize,
}
