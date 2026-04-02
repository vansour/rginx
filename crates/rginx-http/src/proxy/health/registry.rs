use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use hyper::body::{Frame, SizeHint};
use pin_project_lite::pin_project;
use rginx_core::{ConfigSnapshot, Upstream, UpstreamLoadBalance, UpstreamPeer};
use serde::Serialize;

use crate::handler::BoxError;

#[derive(Debug, Clone, Copy)]
struct PeerHealthPolicy {
    unhealthy_after_failures: u32,
    cooldown: Duration,
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

#[derive(Clone)]
pub(crate) struct PeerHealthRegistry {
    policies: Arc<HashMap<String, PeerHealthPolicy>>,
    peers: Arc<HashMap<String, HashMap<String, Arc<PeerHealth>>>>,
}

pub(crate) struct SelectedPeers {
    pub peers: Vec<UpstreamPeer>,
    pub skipped_unhealthy: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PeerStatusSnapshot {
    pub url: String,
    pub weight: u32,
    pub backup: bool,
    pub healthy: bool,
    pub active_requests: u64,
    pub passive_consecutive_failures: u32,
    pub passive_cooldown_remaining_ms: Option<u64>,
    pub active_unhealthy: bool,
    pub active_consecutive_successes: u32,
}

impl PeerHealthPolicy {
    fn from_upstream(upstream: &Upstream) -> Self {
        Self {
            unhealthy_after_failures: upstream.unhealthy_after_failures,
            cooldown: upstream.unhealthy_cooldown,
        }
    }
}

impl PeerHealthRegistry {
    pub(crate) fn from_config(config: &ConfigSnapshot) -> Self {
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

        Self { policies: Arc::new(policies), peers: Arc::new(peers) }
    }

    pub(crate) fn select_peers(
        &self,
        upstream: &Upstream,
        client_ip: std::net::IpAddr,
        limit: usize,
    ) -> SelectedPeers {
        if upstream.load_balance == UpstreamLoadBalance::LeastConn {
            return self.select_peers_by_least_conn(upstream, limit);
        }

        if !upstream.has_primary_peers() {
            return self.select_peers_in_pool(upstream, client_ip, limit, true);
        }

        let primary = self.select_peers_in_pool(upstream, client_ip, limit, false);
        if limit == 0 {
            return SelectedPeers { peers: Vec::new(), skipped_unhealthy: 0 };
        }

        if primary.peers.is_empty() {
            return merge_selected_peers(
                primary,
                self.select_peers_in_pool(upstream, client_ip, limit, true),
            );
        }

        if primary.peers.len() == limit {
            return primary;
        }

        let remaining = limit - primary.peers.len();
        merge_selected_peers(
            primary,
            self.select_peers_in_pool(upstream, client_ip, remaining, true),
        )
    }

    fn select_peers_by_least_conn(&self, upstream: &Upstream, limit: usize) -> SelectedPeers {
        if limit == 0 {
            return SelectedPeers { peers: Vec::new(), skipped_unhealthy: 0 };
        }

        if !upstream.has_primary_peers() {
            return self.select_peers_by_least_conn_in_pool(upstream, limit, true);
        }

        let primary = self.select_peers_by_least_conn_in_pool(upstream, limit, false);
        if primary.peers.is_empty() {
            return merge_selected_peers(
                primary,
                self.select_peers_by_least_conn_in_pool(upstream, limit, true),
            );
        }

        if primary.peers.len() == limit {
            return primary;
        }

        let remaining = limit - primary.peers.len();
        merge_selected_peers(
            primary,
            self.select_peers_by_least_conn_in_pool(upstream, remaining, true),
        )
    }

    fn select_peers_in_pool(
        &self,
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

        self.select_available_peers(upstream, ordered, limit)
    }

    fn select_peers_by_least_conn_in_pool(
        &self,
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

            if self.is_available(&upstream.name, &peer.url) {
                available.push((self.active_requests(&upstream.name, &peer.url), order, peer));
            } else {
                skipped_unhealthy += 1;
            }
        }

        available.sort_by(|left, right| {
            projected_least_conn_load(left.0, left.2.weight, right.0, right.2.weight)
                .then(right.2.weight.cmp(&left.2.weight))
                .then(left.1.cmp(&right.1))
        });

        SelectedPeers {
            peers: available.into_iter().take(limit).map(|(_, _, peer)| peer).collect(),
            skipped_unhealthy,
        }
    }

    fn select_available_peers(
        &self,
        upstream: &Upstream,
        ordered: Vec<UpstreamPeer>,
        limit: usize,
    ) -> SelectedPeers {
        if limit == 0 {
            return SelectedPeers { peers: Vec::new(), skipped_unhealthy: 0 };
        }

        let mut selected = Vec::new();
        let mut skipped_unhealthy = 0;

        for peer in ordered {
            if self.is_available(&upstream.name, &peer.url) {
                selected.push(peer);
                if selected.len() == limit {
                    break;
                }
            } else {
                skipped_unhealthy += 1;
            }
        }

        SelectedPeers { peers: selected, skipped_unhealthy }
    }

    pub(crate) fn record_success(&self, upstream_name: &str, peer_url: &str) -> bool {
        if let Some(health) = self.get(upstream_name, peer_url) {
            return health.record_success();
        }

        false
    }

    pub(crate) fn record_failure(&self, upstream_name: &str, peer_url: &str) -> PeerFailureStatus {
        let Some(policy) = self.policies.get(upstream_name).copied() else {
            return PeerFailureStatus { consecutive_failures: 0, entered_cooldown: false };
        };

        self.get(upstream_name, peer_url)
            .map(|health| health.record_failure(policy))
            .unwrap_or(PeerFailureStatus { consecutive_failures: 0, entered_cooldown: false })
    }

    fn is_available(&self, upstream_name: &str, peer_url: &str) -> bool {
        self.get(upstream_name, peer_url).is_none_or(|health| health.is_available())
    }

    pub(crate) fn record_active_success(
        &self,
        upstream_name: &str,
        peer_url: &str,
        healthy_successes_required: u32,
    ) -> ActiveProbeStatus {
        self.get(upstream_name, peer_url)
            .map(|health| health.record_active_success(healthy_successes_required))
            .unwrap_or(ActiveProbeStatus {
                healthy: true,
                recovered: false,
                consecutive_successes: 0,
            })
    }

    pub(crate) fn record_active_failure(&self, upstream_name: &str, peer_url: &str) -> bool {
        self.get(upstream_name, peer_url).is_some_and(|health| health.record_active_failure())
    }

    pub(crate) fn track_active_request(
        &self,
        upstream_name: &str,
        peer_url: &str,
    ) -> ActivePeerGuard {
        let peer = self.get(upstream_name, peer_url).cloned();
        if let Some(ref peer) = peer {
            peer.increment_active_requests();
        }

        ActivePeerGuard { peer }
    }

    fn active_requests(&self, upstream_name: &str, peer_url: &str) -> u64 {
        self.get(upstream_name, peer_url).map(|health| health.active_requests()).unwrap_or(0)
    }

    fn get(&self, upstream_name: &str, peer_url: &str) -> Option<&Arc<PeerHealth>> {
        self.peers.get(upstream_name).and_then(|upstream_peers| upstream_peers.get(peer_url))
    }

    pub(crate) fn snapshot(
        &self,
        upstream_name: &str,
        peer_url: &str,
        peer_display_url: &str,
        peer_weight: u32,
        peer_backup: bool,
    ) -> PeerStatusSnapshot {
        self.get(upstream_name, peer_url)
            .map(|health| health.snapshot(peer_display_url, peer_weight, peer_backup))
            .unwrap_or_else(|| PeerStatusSnapshot {
                url: peer_display_url.to_string(),
                weight: peer_weight,
                backup: peer_backup,
                healthy: true,
                active_requests: 0,
                passive_consecutive_failures: 0,
                passive_cooldown_remaining_ms: None,
                active_unhealthy: false,
                active_consecutive_successes: 0,
            })
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

    fn increment_active_requests(&self) {
        lock_peer_health(&self.state).active_requests += 1;
    }

    fn decrement_active_requests(&self) {
        let mut state = lock_peer_health(&self.state);
        state.active_requests = state.active_requests.saturating_sub(1);
    }

    fn active_requests(&self) -> u64 {
        lock_peer_health(&self.state).active_requests
    }

    fn snapshot(&self, url: &str, weight: u32, backup: bool) -> PeerStatusSnapshot {
        let state = lock_peer_health(&self.state);
        let now = Instant::now();

        let passive_cooldown_remaining_ms = state
            .passive
            .unhealthy_until
            .and_then(|until| until.checked_duration_since(now))
            .map(|remaining| remaining.as_millis() as u64);
        let healthy = passive_cooldown_remaining_ms.is_none() && !state.active.unhealthy;

        PeerStatusSnapshot {
            url: url.to_string(),
            weight,
            backup,
            healthy,
            active_requests: state.active_requests,
            passive_consecutive_failures: state.passive.consecutive_failures,
            passive_cooldown_remaining_ms,
            active_unhealthy: state.active.unhealthy,
            active_consecutive_successes: state.active.consecutive_successes,
        }
    }
}

pub(crate) struct ActivePeerGuard {
    peer: Option<Arc<PeerHealth>>,
}

impl Drop for ActivePeerGuard {
    fn drop(&mut self) {
        if let Some(peer) = self.peer.take() {
            peer.decrement_active_requests();
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
    use std::sync::Arc;

    use super::{PeerHealth, PeerHealthRegistry};

    #[test]
    fn get_supports_borrowed_upstream_and_peer_lookups() {
        let expected = Arc::new(PeerHealth::default());
        let registry = PeerHealthRegistry {
            policies: Arc::new(HashMap::new()),
            peers: Arc::new(HashMap::from([(
                "backend".to_string(),
                HashMap::from([("http://127.0.0.1:8080".to_string(), expected.clone())]),
            )])),
        };

        let actual = registry
            .get("backend", "http://127.0.0.1:8080")
            .expect("borrowed lookup should find peer");

        assert!(Arc::ptr_eq(actual, &expected));

        let guard = registry.track_active_request("backend", "http://127.0.0.1:8080");
        assert_eq!(registry.active_requests("backend", "http://127.0.0.1:8080"), 1);
        drop(guard);
        assert_eq!(registry.active_requests("backend", "http://127.0.0.1:8080"), 0);
    }
}
