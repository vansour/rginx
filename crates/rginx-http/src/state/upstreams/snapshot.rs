use super::super::tls_runtime::upstream_tls_status_snapshot;
use super::super::*;

impl SharedState {
    pub async fn peer_health_snapshot(&self) -> Vec<UpstreamHealthSnapshot> {
        self.inner.read().await.clients.peer_health_snapshot().await
    }

    pub fn upstream_stats_snapshot(&self) -> Vec<UpstreamStatsSnapshot> {
        self.upstream_stats_snapshot_with_window(None)
    }

    pub fn upstream_stats_snapshot_with_window(
        &self,
        window_secs: Option<u64>,
    ) -> Vec<UpstreamStatsSnapshot> {
        let stats = self.upstream_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut upstream_names = stats.keys().cloned().collect::<Vec<_>>();
        upstream_names.sort();

        upstream_names
            .into_iter()
            .filter_map(|upstream_name| {
                let entry = stats.get(&upstream_name)?;
                let peers = entry
                    .peer_order
                    .iter()
                    .filter_map(|peer_url| {
                        let peer = entry.peers.get(peer_url)?;
                        Some(UpstreamPeerStatsSnapshot {
                            peer_url: peer_url.clone(),
                            attempts_total: peer.attempts_total.load(Ordering::Relaxed),
                            successes_total: peer.successes_total.load(Ordering::Relaxed),
                            failures_total: peer.failures_total.load(Ordering::Relaxed),
                            timeouts_total: peer.timeouts_total.load(Ordering::Relaxed),
                        })
                    })
                    .collect::<Vec<_>>();

                Some(UpstreamStatsSnapshot {
                    upstream_name: upstream_name.clone(),
                    tls: upstream_tls_status_snapshot(entry.upstream.as_ref()),
                    downstream_requests_total: entry
                        .counters
                        .downstream_requests_total
                        .load(Ordering::Relaxed),
                    peer_attempts_total: entry.counters.peer_attempts_total.load(Ordering::Relaxed),
                    peer_successes_total: entry
                        .counters
                        .peer_successes_total
                        .load(Ordering::Relaxed),
                    peer_failures_total: entry.counters.peer_failures_total.load(Ordering::Relaxed),
                    peer_timeouts_total: entry.counters.peer_timeouts_total.load(Ordering::Relaxed),
                    failovers_total: entry.counters.failovers_total.load(Ordering::Relaxed),
                    completed_responses_total: entry
                        .counters
                        .completed_responses_total
                        .load(Ordering::Relaxed),
                    bad_gateway_responses_total: entry
                        .counters
                        .bad_gateway_responses_total
                        .load(Ordering::Relaxed),
                    gateway_timeout_responses_total: entry
                        .counters
                        .gateway_timeout_responses_total
                        .load(Ordering::Relaxed),
                    bad_request_responses_total: entry
                        .counters
                        .bad_request_responses_total
                        .load(Ordering::Relaxed),
                    payload_too_large_responses_total: entry
                        .counters
                        .payload_too_large_responses_total
                        .load(Ordering::Relaxed),
                    unsupported_media_type_responses_total: entry
                        .counters
                        .unsupported_media_type_responses_total
                        .load(Ordering::Relaxed),
                    no_healthy_peers_total: entry
                        .counters
                        .no_healthy_peers_total
                        .load(Ordering::Relaxed),
                    tls_failures_unknown_ca_total: entry
                        .counters
                        .tls_failures_unknown_ca_total
                        .load(Ordering::Relaxed),
                    tls_failures_bad_certificate_total: entry
                        .counters
                        .tls_failures_bad_certificate_total
                        .load(Ordering::Relaxed),
                    tls_failures_certificate_revoked_total: entry
                        .counters
                        .tls_failures_certificate_revoked_total
                        .load(Ordering::Relaxed),
                    tls_failures_verify_depth_exceeded_total: entry
                        .counters
                        .tls_failures_verify_depth_exceeded_total
                        .load(Ordering::Relaxed),
                    recent_60s: entry.counters.recent_60s.snapshot(),
                    recent_window: window_secs.map(|window_secs| {
                        entry.counters.recent_60s.snapshot_for_window(window_secs)
                    }),
                    peers,
                })
            })
            .collect()
    }

    pub(crate) fn sync_upstream_stats(&self, config: &ConfigSnapshot) {
        let existing = self.upstream_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let next = build_upstream_stats_map(config, Some(&*existing));
        drop(existing);
        *self.upstream_stats.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
        let existing = self
            .upstream_component_versions
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let next = build_upstream_name_versions(config, Some(&*existing));
        drop(existing);
        *self
            .upstream_component_versions
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
    }

    pub(crate) fn sync_peer_health_versions(&self, config: &ConfigSnapshot) {
        let existing = self
            .peer_health_component_versions
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let next = build_upstream_name_versions(config, Some(&*existing));
        drop(existing);
        *self
            .peer_health_component_versions
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
    }

    pub(super) fn upstream_stats_counters(
        &self,
        upstream_name: &str,
    ) -> Option<Arc<UpstreamStats>> {
        let stats = self.upstream_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        stats.get(upstream_name).map(|entry| entry.counters.clone())
    }

    pub(super) fn upstream_stats_peer_counters(
        &self,
        upstream_name: &str,
        peer_url: &str,
    ) -> Option<(Arc<UpstreamStats>, Arc<UpstreamPeerStats>)> {
        let stats = self.upstream_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = stats.get(upstream_name)?;
        let peer = entry.peers.get(peer_url)?.clone();
        Some((entry.counters.clone(), peer))
    }

    pub(crate) fn mark_all_upstream_targets_changed(
        &self,
        previous: &ConfigSnapshot,
        next: &ConfigSnapshot,
        version: u64,
    ) {
        let mut upstream_versions = self
            .upstream_component_versions
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for name in previous.upstreams.keys() {
            upstream_versions.insert(name.clone(), version);
        }
        for name in next.upstreams.keys() {
            upstream_versions.insert(name.clone(), version);
        }

        let mut peer_health_versions = self
            .peer_health_component_versions
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for name in previous.upstreams.keys() {
            peer_health_versions.insert(name.clone(), version);
        }
        for name in next.upstreams.keys() {
            peer_health_versions.insert(name.clone(), version);
        }
    }
}
