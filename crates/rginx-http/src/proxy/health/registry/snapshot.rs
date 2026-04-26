use super::*;

use super::state::lock_peer_health;

impl PeerHealthRegistry {
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
}

impl PeerHealth {
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
