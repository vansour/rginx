use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use hyper::body::{Frame, SizeHint};
use pin_project_lite::pin_project;
use rginx_core::{ConfigSnapshot, Upstream, UpstreamLoadBalance, UpstreamPeer};
use serde::{Deserialize, Serialize};

use crate::handler::BoxError;
use crate::proxy::clients::ProxyClient;
use crate::proxy::{HealthChangeNotifier, ResolvedUpstreamPeer, UpstreamResolverRuntimeSnapshot};

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

impl PeerHealthPolicy {
    fn from_upstream(upstream: &Upstream) -> Self {
        Self {
            unhealthy_after_failures: upstream.unhealthy_after_failures,
            cooldown: upstream.unhealthy_cooldown,
            active_health_enabled: upstream.active_health_check.is_some(),
        }
    }
}

impl PeerHealthRegistry {
    pub(crate) fn from_config(config: &ConfigSnapshot) -> Self {
        Self::from_config_with_notifier(config, None)
    }

    pub(crate) fn from_config_with_notifier(
        config: &ConfigSnapshot,
        notifier: Option<HealthChangeNotifier>,
    ) -> Self {
        let policies = config
            .upstreams
            .iter()
            .map(|(upstream_name, upstream)| {
                (upstream_name.clone(), PeerHealthPolicy::from_upstream(upstream.as_ref()))
            })
            .collect::<HashMap<_, _>>();
        let peers = config
            .upstreams
            .iter()
            .map(|(upstream_name, upstream)| {
                let peers = upstream
                    .peers
                    .iter()
                    .map(|peer| (peer.url.clone(), Arc::new(PeerHealth::default())))
                    .collect::<HashMap<_, _>>();
                (upstream_name.clone(), peers)
            })
            .collect::<HashMap<_, _>>();
        let endpoint_peers = config
            .upstreams
            .keys()
            .map(|upstream_name| (upstream_name.clone(), HashMap::new()))
            .collect::<HashMap<_, _>>();

        Self {
            policies: Arc::new(policies),
            peers: Arc::new(peers),
            endpoint_peers: Arc::new(Mutex::new(endpoint_peers)),
            notifier,
        }
    }

    pub(crate) async fn select_peers(
        &self,
        client: &ProxyClient,
        upstream: &Upstream,
        client_ip: std::net::IpAddr,
        limit: usize,
    ) -> SelectedPeers {
        if upstream.load_balance == UpstreamLoadBalance::LeastConn {
            return self.select_peers_by_least_conn(client, upstream, limit).await;
        }

        if !upstream.has_primary_peers() {
            return self.select_peers_in_pool(client, upstream, client_ip, limit, true).await;
        }

        let primary = self.select_peers_in_pool(client, upstream, client_ip, limit, false).await;
        if limit == 0 {
            return SelectedPeers { peers: Vec::new(), skipped_unhealthy: 0 };
        }

        if primary.peers.is_empty() {
            return merge_selected_peers(
                primary,
                self.select_peers_in_pool(client, upstream, client_ip, limit, true).await,
            );
        }

        if primary.peers.len() == limit {
            return primary;
        }

        let remaining = limit - primary.peers.len();
        merge_selected_peers(
            primary,
            self.select_peers_in_pool(client, upstream, client_ip, remaining, true).await,
        )
    }

    async fn select_peers_by_least_conn(
        &self,
        client: &ProxyClient,
        upstream: &Upstream,
        limit: usize,
    ) -> SelectedPeers {
        if limit == 0 {
            return SelectedPeers { peers: Vec::new(), skipped_unhealthy: 0 };
        }

        if !upstream.has_primary_peers() {
            return self.select_peers_by_least_conn_in_pool(client, upstream, limit, true).await;
        }

        let primary = self.select_peers_by_least_conn_in_pool(client, upstream, limit, false).await;
        if primary.peers.is_empty() {
            return merge_selected_peers(
                primary,
                self.select_peers_by_least_conn_in_pool(client, upstream, limit, true).await,
            );
        }

        if primary.peers.len() == limit {
            return primary;
        }

        let remaining = limit - primary.peers.len();
        merge_selected_peers(
            primary,
            self.select_peers_by_least_conn_in_pool(client, upstream, remaining, true).await,
        )
    }

    async fn select_peers_in_pool(
        &self,
        client: &ProxyClient,
        upstream: &Upstream,
        client_ip: std::net::IpAddr,
        limit: usize,
        backup: bool,
    ) -> SelectedPeers {
        let ordered = if backup {
            upstream.backup_peers_for_client_ip(client_ip, upstream.peers.len())
        } else {
            upstream.primary_peers_for_client_ip(client_ip, upstream.peers.len())
        };

        self.select_available_peers(client, upstream, ordered, limit).await
    }

    async fn select_peers_by_least_conn_in_pool(
        &self,
        client: &ProxyClient,
        upstream: &Upstream,
        limit: usize,
        backup: bool,
    ) -> SelectedPeers {
        let mut available = Vec::new();
        let mut skipped_unhealthy = 0;

        for (order, peer) in upstream.peers.iter().cloned().enumerate() {
            if peer.backup != backup {
                continue;
            }

            let endpoints: Vec<ResolvedUpstreamPeer> =
                client.resolve_peer(&peer).await.unwrap_or_default();
            for endpoint in endpoints {
                self.ensure_endpoint(
                    &upstream.name,
                    &endpoint.endpoint_key,
                    &endpoint.logical_peer_url,
                );
                if self.is_available(&upstream.name, &endpoint.endpoint_key) {
                    available.push((
                        self.active_requests(&upstream.name, &endpoint.endpoint_key),
                        order,
                        endpoint,
                    ));
                } else {
                    skipped_unhealthy += 1;
                }
            }
        }

        available.sort_by(|left, right| {
            projected_least_conn_load(left.0, left.2.weight, right.0, right.2.weight)
                .then(right.2.weight.cmp(&left.2.weight))
                .then(left.1.cmp(&right.1))
                .then(left.2.dial_authority.cmp(&right.2.dial_authority))
        });

        SelectedPeers {
            peers: available.into_iter().take(limit).map(|(_, _, peer)| peer).collect(),
            skipped_unhealthy,
        }
    }

    async fn select_available_peers(
        &self,
        client: &ProxyClient,
        upstream: &Upstream,
        ordered: Vec<UpstreamPeer>,
        limit: usize,
    ) -> SelectedPeers {
        if limit == 0 {
            return SelectedPeers { peers: Vec::new(), skipped_unhealthy: 0 };
        }

        let mut batches = Vec::new();
        let mut skipped_unhealthy = 0;

        for peer in ordered {
            let mut endpoints: Vec<ResolvedUpstreamPeer> =
                client.resolve_peer(&peer).await.unwrap_or_default();
            for endpoint in &endpoints {
                self.ensure_endpoint(
                    &upstream.name,
                    &endpoint.endpoint_key,
                    &endpoint.logical_peer_url,
                );
            }
            endpoints.sort_by(|left, right| {
                self.active_requests(&upstream.name, &left.endpoint_key)
                    .cmp(&self.active_requests(&upstream.name, &right.endpoint_key))
                    .then(left.dial_authority.cmp(&right.dial_authority))
            });

            let mut available = Vec::new();
            for endpoint in endpoints {
                self.ensure_endpoint(
                    &upstream.name,
                    &endpoint.endpoint_key,
                    &endpoint.logical_peer_url,
                );
                if self.is_available(&upstream.name, &endpoint.endpoint_key) {
                    available.push(endpoint);
                } else {
                    skipped_unhealthy += 1;
                }
            }
            if !available.is_empty() {
                batches.push(available);
            }
        }

        let mut selected = Vec::new();
        let mut depth = 0usize;
        while selected.len() < limit {
            let mut advanced = false;
            for batch in &batches {
                if let Some(endpoint) = batch.get(depth) {
                    selected.push(endpoint.clone());
                    advanced = true;
                    if selected.len() == limit {
                        break;
                    }
                }
            }
            if !advanced {
                break;
            }
            depth += 1;
        }

        SelectedPeers { peers: selected, skipped_unhealthy }
    }

    pub(crate) fn record_success(&self, upstream_name: &str, peer_url: &str) -> bool {
        if let Some(health) = self.get_health(upstream_name, peer_url) {
            let recovered = health.record_success();
            self.notify_change(upstream_name);
            return recovered;
        }

        false
    }

    pub(crate) fn record_failure(&self, upstream_name: &str, peer_url: &str) -> PeerFailureStatus {
        let Some(policy) = self.policies.get(upstream_name).copied() else {
            return PeerFailureStatus { consecutive_failures: 0, entered_cooldown: false };
        };

        self.get_health(upstream_name, peer_url)
            .map(|health| {
                let status = health.record_failure(policy);
                self.notify_change(upstream_name);
                status
            })
            .unwrap_or(PeerFailureStatus { consecutive_failures: 0, entered_cooldown: false })
    }

    fn is_available(&self, upstream_name: &str, peer_url: &str) -> bool {
        self.get_health(upstream_name, peer_url).is_none_or(|health| health.is_available())
    }

    pub(crate) fn record_active_success(
        &self,
        upstream_name: &str,
        peer_url: &str,
        healthy_successes_required: u32,
    ) -> ActiveProbeStatus {
        self.get_health(upstream_name, peer_url)
            .map(|health| {
                let status = health.record_active_success(healthy_successes_required);
                self.notify_change(upstream_name);
                status
            })
            .unwrap_or(ActiveProbeStatus {
                healthy: true,
                recovered: false,
                consecutive_successes: 0,
            })
    }

    pub(crate) fn record_active_failure(&self, upstream_name: &str, peer_url: &str) -> bool {
        self.get_health(upstream_name, peer_url).is_some_and(|health| {
            let changed = health.record_active_failure();
            self.notify_change(upstream_name);
            changed
        })
    }

    pub(crate) fn track_active_request(
        &self,
        upstream_name: &str,
        peer_url: &str,
    ) -> ActivePeerGuard {
        let peer = self.get_health(upstream_name, peer_url);
        if let Some(ref peer) = peer {
            let transitioned_from_idle = peer.increment_active_requests();
            if transitioned_from_idle {
                self.notify_change(upstream_name);
            }
        }

        ActivePeerGuard {
            peer,
            notifier: self.notifier.clone(),
            upstream_name: upstream_name.to_string(),
        }
    }

    fn active_requests(&self, upstream_name: &str, peer_url: &str) -> u64 {
        self.get_health(upstream_name, peer_url).map(|health| health.active_requests()).unwrap_or(0)
    }

    fn get_health(&self, upstream_name: &str, peer_url: &str) -> Option<Arc<PeerHealth>> {
        self.get_endpoint(upstream_name, peer_url)
            .or_else(|| self.get_logical_peer(upstream_name, peer_url))
    }

    fn get_logical_peer(&self, upstream_name: &str, peer_url: &str) -> Option<Arc<PeerHealth>> {
        self.peers
            .get(upstream_name)
            .and_then(|upstream_peers| upstream_peers.get(peer_url))
            .cloned()
    }

    fn get_endpoint(&self, upstream_name: &str, peer_url: &str) -> Option<Arc<PeerHealth>> {
        self.endpoint_peers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(upstream_name)
            .and_then(|upstream_peers| upstream_peers.get(peer_url))
            .cloned()
    }

    fn ensure_endpoint(
        &self,
        upstream_name: &str,
        endpoint_key: &str,
        logical_peer_url: &str,
    ) -> Arc<PeerHealth> {
        let mut endpoints =
            self.endpoint_peers.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        endpoints
            .entry(upstream_name.to_string())
            .or_default()
            .entry(endpoint_key.to_string())
            .or_insert_with(|| {
                if endpoint_key == logical_peer_url {
                    self.get_logical_peer(upstream_name, logical_peer_url)
                        .unwrap_or_else(|| Arc::new(PeerHealth::default()))
                } else {
                    Arc::new(PeerHealth::default())
                }
            })
            .clone()
    }

    pub(crate) fn snapshot_for_upstream(
        &self,
        upstream: &Upstream,
        resolver: UpstreamResolverRuntimeSnapshot,
        endpoints: Vec<ResolvedUpstreamPeer>,
    ) -> UpstreamHealthSnapshot {
        let policy = self
            .policies
            .get(&upstream.name)
            .copied()
            .unwrap_or_else(|| PeerHealthPolicy::from_upstream(upstream));

        let endpoint_snapshots = endpoints
            .iter()
            .map(|endpoint| {
                self.get_health(&upstream.name, &endpoint.endpoint_key)
                    .map(|health| health.snapshot_endpoint(endpoint))
                    .unwrap_or_else(|| default_endpoint_snapshot(endpoint))
            })
            .collect::<Vec<_>>();

        let peer_snapshots = upstream
            .peers
            .iter()
            .map(|peer| {
                let peer_endpoints = endpoint_snapshots
                    .iter()
                    .filter(|endpoint| endpoint.logical_peer_url == peer.url)
                    .cloned()
                    .collect::<Vec<_>>();
                if !peer_endpoints.is_empty() {
                    aggregate_peer_snapshot(peer, &peer_endpoints)
                } else {
                    self.peers
                        .get(&upstream.name)
                        .and_then(|upstream_peers| upstream_peers.get(&peer.url))
                        .map(|health| health.snapshot(peer))
                        .unwrap_or_else(|| default_peer_snapshot(peer, !peer_is_hostname(peer)))
                }
            })
            .collect::<Vec<_>>();

        UpstreamHealthSnapshot {
            upstream_name: upstream.name.clone(),
            unhealthy_after_failures: policy.unhealthy_after_failures,
            cooldown_ms: policy.cooldown.as_millis().min(u128::from(u64::MAX)) as u64,
            active_health_enabled: policy.active_health_enabled,
            resolver,
            peers: peer_snapshots,
            endpoints: endpoint_snapshots,
        }
    }

    fn notify_change(&self, upstream_name: &str) {
        if let Some(notifier) = &self.notifier {
            notifier(upstream_name);
        }
    }
}

impl PeerHealth {
    fn is_available(&self) -> bool {
        let state = lock_peer_health(&self.state);
        let passive_available =
            state.passive.unhealthy_until.is_none_or(|until| until <= Instant::now());
        passive_available && !state.active.unhealthy
    }

    fn record_success(&self) -> bool {
        let mut state = lock_peer_health(&self.state);
        let recovered = state.passive.pending_recovery;
        state.passive = PassiveHealthState::default();
        recovered
    }

    fn record_failure(&self, policy: PeerHealthPolicy) -> PeerFailureStatus {
        let mut state = lock_peer_health(&self.state);
        let now = Instant::now();

        if state.passive.unhealthy_until.is_some_and(|until| until <= now) {
            state.passive.unhealthy_until = None;
            state.passive.consecutive_failures = 0;
        }

        let already_in_cooldown = state.passive.unhealthy_until.is_some_and(|until| until > now);
        state.passive.consecutive_failures += 1;
        let entered_cooldown = !already_in_cooldown
            && state.passive.consecutive_failures >= policy.unhealthy_after_failures;
        if entered_cooldown {
            state.passive.unhealthy_until = Some(now + policy.cooldown);
            state.passive.pending_recovery = true;
        } else if already_in_cooldown {
            state.passive.unhealthy_until = Some(now + policy.cooldown);
        }

        PeerFailureStatus {
            consecutive_failures: state.passive.consecutive_failures,
            entered_cooldown,
        }
    }

    fn record_active_success(&self, healthy_successes_required: u32) -> ActiveProbeStatus {
        let mut state = lock_peer_health(&self.state);
        if !state.active.unhealthy {
            return ActiveProbeStatus { healthy: true, recovered: false, consecutive_successes: 0 };
        }

        state.active.consecutive_successes += 1;
        let consecutive_successes = state.active.consecutive_successes;
        let recovered = consecutive_successes >= healthy_successes_required;
        if recovered {
            state.active = ActiveHealthState::default();
        }

        ActiveProbeStatus { healthy: recovered, recovered, consecutive_successes }
    }

    fn record_active_failure(&self) -> bool {
        let mut state = lock_peer_health(&self.state);
        let was_healthy = !state.active.unhealthy;
        state.active.unhealthy = true;
        state.active.consecutive_successes = 0;
        was_healthy
    }

    fn increment_active_requests(&self) -> bool {
        let mut state = lock_peer_health(&self.state);
        let transitioned_from_idle = state.active_requests == 0;
        state.active_requests += 1;
        transitioned_from_idle
    }

    fn decrement_active_requests(&self) -> bool {
        let mut state = lock_peer_health(&self.state);
        let was_active = state.active_requests > 0;
        state.active_requests = state.active_requests.saturating_sub(1);
        was_active && state.active_requests == 0
    }

    fn active_requests(&self) -> u64 {
        lock_peer_health(&self.state).active_requests
    }

    fn snapshot(&self, peer: &UpstreamPeer) -> PeerHealthSnapshot {
        let now = Instant::now();
        let state = lock_peer_health(&self.state);
        let passive_cooldown_remaining_ms = state
            .passive
            .unhealthy_until
            .and_then(|until| until.checked_duration_since(now))
            .map(|remaining| remaining.as_millis().min(u128::from(u64::MAX)) as u64);
        let passive_available = state.passive.unhealthy_until.is_none_or(|until| until <= now);

        PeerHealthSnapshot {
            peer_url: peer.url.clone(),
            backup: peer.backup,
            weight: peer.weight,
            available: passive_available && !state.active.unhealthy,
            passive_consecutive_failures: state.passive.consecutive_failures,
            passive_cooldown_remaining_ms,
            passive_pending_recovery: state.passive.pending_recovery,
            active_unhealthy: state.active.unhealthy,
            active_consecutive_successes: state.active.consecutive_successes,
            active_requests: state.active_requests,
        }
    }

    fn snapshot_endpoint(&self, endpoint: &ResolvedUpstreamPeer) -> ResolvedEndpointHealthSnapshot {
        let now = Instant::now();
        let state = lock_peer_health(&self.state);
        let passive_cooldown_remaining_ms = state
            .passive
            .unhealthy_until
            .and_then(|until| until.checked_duration_since(now))
            .map(|remaining| remaining.as_millis().min(u128::from(u64::MAX)) as u64);
        let passive_available = state.passive.unhealthy_until.is_none_or(|until| until <= now);

        ResolvedEndpointHealthSnapshot {
            endpoint_key: endpoint.endpoint_key.clone(),
            logical_peer_url: endpoint.logical_peer_url.clone(),
            display_url: endpoint.display_url.clone(),
            dial_addr: endpoint.socket_addr.to_string(),
            server_name: endpoint.server_name.clone(),
            backup: endpoint.backup,
            weight: endpoint.weight,
            available: passive_available && !state.active.unhealthy,
            passive_consecutive_failures: state.passive.consecutive_failures,
            passive_cooldown_remaining_ms,
            passive_pending_recovery: state.passive.pending_recovery,
            active_unhealthy: state.active.unhealthy,
            active_consecutive_successes: state.active.consecutive_successes,
            active_requests: state.active_requests,
        }
    }
}

fn default_peer_snapshot(peer: &UpstreamPeer, available: bool) -> PeerHealthSnapshot {
    PeerHealthSnapshot {
        peer_url: peer.url.clone(),
        backup: peer.backup,
        weight: peer.weight,
        available,
        passive_consecutive_failures: 0,
        passive_cooldown_remaining_ms: None,
        passive_pending_recovery: false,
        active_unhealthy: false,
        active_consecutive_successes: 0,
        active_requests: 0,
    }
}

fn default_endpoint_snapshot(endpoint: &ResolvedUpstreamPeer) -> ResolvedEndpointHealthSnapshot {
    ResolvedEndpointHealthSnapshot {
        endpoint_key: endpoint.endpoint_key.clone(),
        logical_peer_url: endpoint.logical_peer_url.clone(),
        display_url: endpoint.display_url.clone(),
        dial_addr: endpoint.socket_addr.to_string(),
        server_name: endpoint.server_name.clone(),
        backup: endpoint.backup,
        weight: endpoint.weight,
        available: true,
        passive_consecutive_failures: 0,
        passive_cooldown_remaining_ms: None,
        passive_pending_recovery: false,
        active_unhealthy: false,
        active_consecutive_successes: 0,
        active_requests: 0,
    }
}

fn aggregate_peer_snapshot(
    peer: &UpstreamPeer,
    endpoints: &[ResolvedEndpointHealthSnapshot],
) -> PeerHealthSnapshot {
    let available = endpoints.iter().any(|endpoint| endpoint.available);
    let passive_consecutive_failures =
        endpoints.iter().map(|endpoint| endpoint.passive_consecutive_failures).max().unwrap_or(0);
    let passive_cooldown_remaining_ms =
        endpoints.iter().filter_map(|endpoint| endpoint.passive_cooldown_remaining_ms).max();
    let passive_pending_recovery =
        endpoints.iter().any(|endpoint| endpoint.passive_pending_recovery);
    let active_unhealthy = endpoints.iter().all(|endpoint| endpoint.active_unhealthy);
    let active_consecutive_successes =
        endpoints.iter().map(|endpoint| endpoint.active_consecutive_successes).max().unwrap_or(0);
    let active_requests = endpoints.iter().map(|endpoint| endpoint.active_requests).sum();

    PeerHealthSnapshot {
        peer_url: peer.url.clone(),
        backup: peer.backup,
        weight: peer.weight,
        available,
        passive_consecutive_failures,
        passive_cooldown_remaining_ms,
        passive_pending_recovery,
        active_unhealthy,
        active_consecutive_successes,
        active_requests,
    }
}

fn peer_is_hostname(peer: &UpstreamPeer) -> bool {
    peer.url
        .parse::<http::Uri>()
        .ok()
        .and_then(|uri| uri.host().map(str::to_string))
        .is_some_and(|host| host.parse::<std::net::IpAddr>().is_err())
}

pub(crate) struct ActivePeerGuard {
    peer: Option<Arc<PeerHealth>>,
    notifier: Option<HealthChangeNotifier>,
    upstream_name: String,
}

impl Drop for ActivePeerGuard {
    fn drop(&mut self) {
        if let Some(peer) = self.peer.take() {
            let transitioned_to_idle = peer.decrement_active_requests();
            if transitioned_to_idle && let Some(notifier) = &self.notifier {
                notifier(&self.upstream_name);
            }
        }
    }
}

pin_project! {
    pub(crate) struct ActivePeerBody<B> {
        #[pin]
        inner: B,
        guard: Option<ActivePeerGuard>,
    }
}

impl<B> ActivePeerBody<B> {
    pub(crate) fn new(inner: B, guard: ActivePeerGuard) -> Self {
        Self { inner, guard: Some(guard) }
    }
}

impl<B> hyper::body::Body for ActivePeerBody<B>
where
    B: hyper::body::Body<Data = Bytes>,
    B::Error: Into<BoxError>,
{
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();
        match this.inner.as_mut().poll_frame(cx) {
            std::task::Poll::Ready(Some(Ok(frame))) => std::task::Poll::Ready(Some(Ok(frame))),
            std::task::Poll::Ready(Some(Err(error))) => {
                this.guard.take();
                std::task::Poll::Ready(Some(Err(error.into())))
            }
            std::task::Poll::Ready(None) => {
                this.guard.take();
                std::task::Poll::Ready(None)
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

fn lock_peer_health(state: &Mutex<PeerHealthState>) -> std::sync::MutexGuard<'_, PeerHealthState> {
    state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn merge_selected_peers(mut primary: SelectedPeers, secondary: SelectedPeers) -> SelectedPeers {
    primary.skipped_unhealthy += secondary.skipped_unhealthy;
    primary.peers.extend(secondary.peers);
    primary
}

fn projected_least_conn_load(
    left_active_requests: u64,
    left_weight: u32,
    right_active_requests: u64,
    right_weight: u32,
) -> std::cmp::Ordering {
    let left = u128::from(left_active_requests.saturating_add(1)) * u128::from(right_weight.max(1));
    let right =
        u128::from(right_active_requests.saturating_add(1)) * u128::from(left_weight.max(1));
    left.cmp(&right)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::{IpAddr, SocketAddr};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use rginx_core::{
        ActiveHealthCheck, Upstream, UpstreamDnsPolicy, UpstreamLoadBalance, UpstreamPeer,
        UpstreamProtocol, UpstreamSettings, UpstreamTls,
    };

    use super::{PeerHealth, PeerHealthRegistry, UpstreamResolverRuntimeSnapshot};
    use crate::proxy::ResolvedUpstreamPeer;

    fn resolved_peer(peer: &UpstreamPeer) -> ResolvedUpstreamPeer {
        let socket_addr = peer
            .authority
            .parse::<SocketAddr>()
            .unwrap_or_else(|_| SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 80));
        ResolvedUpstreamPeer {
            url: peer.url.clone(),
            logical_peer_url: peer.url.clone(),
            endpoint_key: peer.url.clone(),
            display_url: peer.url.clone(),
            scheme: peer.scheme.clone(),
            upstream_authority: peer.authority.clone(),
            dial_authority: peer.authority.clone(),
            socket_addr,
            server_name: socket_addr.ip().to_string(),
            weight: peer.weight,
            backup: peer.backup,
        }
    }

    #[test]
    fn get_supports_borrowed_upstream_and_peer_lookups() {
        let expected = Arc::new(PeerHealth::default());
        let registry = PeerHealthRegistry {
            policies: Arc::new(HashMap::new()),
            peers: Arc::new(HashMap::from([(
                "backend".to_string(),
                HashMap::from([("http://127.0.0.1:8080".to_string(), expected.clone())]),
            )])),
            endpoint_peers: Arc::new(std::sync::Mutex::new(HashMap::from([(
                "backend".to_string(),
                HashMap::from([("http://127.0.0.1:8080".to_string(), expected.clone())]),
            )]))),
            notifier: None,
        };

        let actual = registry
            .get_endpoint("backend", "http://127.0.0.1:8080")
            .expect("borrowed lookup should find peer");

        assert!(Arc::ptr_eq(&actual, &expected));

        let guard = registry.track_active_request("backend", "http://127.0.0.1:8080");
        assert_eq!(registry.active_requests("backend", "http://127.0.0.1:8080"), 1);
        drop(guard);
        assert_eq!(registry.active_requests("backend", "http://127.0.0.1:8080"), 0);
    }

    #[test]
    fn snapshot_reports_passive_and_active_health_state() {
        let upstream = Upstream::new(
            "backend".to_string(),
            vec![UpstreamPeer {
                url: "http://127.0.0.1:8080".to_string(),
                scheme: "http".to_string(),
                authority: "127.0.0.1:8080".to_string(),
                weight: 2,
                backup: true,
            }],
            UpstreamTls::NativeRoots,
            UpstreamSettings {
                protocol: UpstreamProtocol::Auto,
                load_balance: UpstreamLoadBalance::RoundRobin,
                dns: UpstreamDnsPolicy::default(),
                server_name: true,
                server_name_override: None,
                tls_versions: None,
                server_verify_depth: None,
                server_crl_path: None,
                client_identity: None,
                request_timeout: Duration::from_secs(30),
                connect_timeout: Duration::from_secs(30),
                write_timeout: Duration::from_secs(30),
                idle_timeout: Duration::from_secs(30),
                pool_idle_timeout: Some(Duration::from_secs(90)),
                pool_max_idle_per_host: usize::MAX,
                tcp_keepalive: None,
                tcp_nodelay: false,
                http2_keep_alive_interval: None,
                http2_keep_alive_timeout: Duration::from_secs(20),
                http2_keep_alive_while_idle: false,
                max_replayable_request_body_bytes: 64 * 1024,
                unhealthy_after_failures: 1,
                unhealthy_cooldown: Duration::from_secs(30),
                active_health_check: Some(ActiveHealthCheck {
                    path: "/healthz".to_string(),
                    grpc_service: None,
                    interval: Duration::from_secs(5),
                    timeout: Duration::from_secs(2),
                    healthy_successes_required: 2,
                }),
            },
        );
        let server = rginx_core::Server {
            listen_addr: "127.0.0.1:8080".parse().unwrap(),
            default_certificate: None,
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
        };
        let config = rginx_core::ConfigSnapshot {
            runtime: rginx_core::RuntimeSettings {
                shutdown_timeout: Duration::from_secs(1),
                worker_threads: None,
                accept_workers: 1,
            },
            listeners: vec![rginx_core::Listener {
                id: "default".to_string(),
                name: "default".to_string(),
                server,
                tls_termination_enabled: false,
                proxy_protocol_enabled: false,
                http3: None,
            }],
            default_vhost: rginx_core::VirtualHost {
                id: "server".to_string(),
                server_names: Vec::new(),
                routes: Vec::new(),
                tls: None,
            },
            vhosts: Vec::new(),
            upstreams: HashMap::from([("backend".to_string(), Arc::new(upstream))]),
        };

        let registry = PeerHealthRegistry::from_config(&config);
        let guard = registry.track_active_request("backend", "http://127.0.0.1:8080");
        let failure = registry.record_failure("backend", "http://127.0.0.1:8080");
        assert!(failure.entered_cooldown);
        assert!(registry.record_active_failure("backend", "http://127.0.0.1:8080"));

        let upstream = config.upstreams["backend"].as_ref();
        let snapshots = [registry.snapshot_for_upstream(
            upstream,
            UpstreamResolverRuntimeSnapshot::default(),
            vec![resolved_peer(&upstream.peers[0])],
        )];
        assert_eq!(snapshots.len(), 1);
        let upstream = &snapshots[0];
        assert_eq!(upstream.upstream_name, "backend");
        assert_eq!(upstream.unhealthy_after_failures, 1);
        assert_eq!(upstream.cooldown_ms, 30_000);
        assert!(upstream.active_health_enabled);
        assert_eq!(upstream.peers.len(), 1);
        let peer = &upstream.peers[0];
        assert_eq!(peer.peer_url, "http://127.0.0.1:8080");
        assert!(peer.backup);
        assert_eq!(peer.weight, 2);
        assert!(!peer.available);
        assert_eq!(peer.passive_consecutive_failures, 1);
        assert!(peer.passive_cooldown_remaining_ms.is_some());
        assert!(peer.passive_pending_recovery);
        assert!(peer.active_unhealthy);
        assert_eq!(peer.active_consecutive_successes, 0);
        assert_eq!(peer.active_requests, 1);

        drop(guard);
    }

    #[test]
    fn track_active_request_only_notifies_when_peer_becomes_active_or_idle() {
        let notifications = Arc::new(AtomicUsize::new(0));
        let registry = PeerHealthRegistry {
            policies: Arc::new(HashMap::new()),
            peers: Arc::new(HashMap::from([(
                "backend".to_string(),
                HashMap::from([(
                    "http://127.0.0.1:8080".to_string(),
                    Arc::new(PeerHealth::default()),
                )]),
            )])),
            endpoint_peers: Arc::new(std::sync::Mutex::new(HashMap::from([(
                "backend".to_string(),
                HashMap::from([(
                    "http://127.0.0.1:8080".to_string(),
                    Arc::new(PeerHealth::default()),
                )]),
            )]))),
            notifier: Some(Arc::new({
                let notifications = notifications.clone();
                move |_upstream_name| {
                    notifications.fetch_add(1, Ordering::Relaxed);
                }
            })),
        };

        let first = registry.track_active_request("backend", "http://127.0.0.1:8080");
        assert_eq!(notifications.load(Ordering::Relaxed), 1);

        let second = registry.track_active_request("backend", "http://127.0.0.1:8080");
        assert_eq!(notifications.load(Ordering::Relaxed), 1);

        drop(first);
        assert_eq!(notifications.load(Ordering::Relaxed), 1);

        drop(second);
        assert_eq!(notifications.load(Ordering::Relaxed), 2);
    }
}
