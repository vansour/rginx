use super::tls_runtime::upstream_tls_status_snapshot;
use super::*;

impl SharedState {
    pub async fn peer_health_snapshot(&self) -> Vec<UpstreamHealthSnapshot> {
        self.inner.read().await.clients.peer_health_snapshot()
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

    pub(crate) fn record_upstream_request(&self, upstream_name: &str) {
        let Some(counters) = self.upstream_stats_counters(upstream_name) else {
            return;
        };
        counters.downstream_requests_total.fetch_add(1, Ordering::Relaxed);
        counters.recent_60s.downstream_requests_total.increment_now();
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_peer_attempt(&self, upstream_name: &str, peer_url: &str) {
        let Some((counters, peer)) = self.upstream_stats_peer_counters(upstream_name, peer_url)
        else {
            return;
        };
        counters.peer_attempts_total.fetch_add(1, Ordering::Relaxed);
        peer.attempts_total.fetch_add(1, Ordering::Relaxed);
        counters.recent_60s.peer_attempts_total.increment_now();
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_peer_success(&self, upstream_name: &str, peer_url: &str) {
        let Some((counters, peer)) = self.upstream_stats_peer_counters(upstream_name, peer_url)
        else {
            return;
        };
        counters.peer_successes_total.fetch_add(1, Ordering::Relaxed);
        peer.successes_total.fetch_add(1, Ordering::Relaxed);
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_peer_failure(&self, upstream_name: &str, peer_url: &str) {
        let Some((counters, peer)) = self.upstream_stats_peer_counters(upstream_name, peer_url)
        else {
            return;
        };
        counters.peer_failures_total.fetch_add(1, Ordering::Relaxed);
        peer.failures_total.fetch_add(1, Ordering::Relaxed);
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_peer_timeout(&self, upstream_name: &str, peer_url: &str) {
        let Some((counters, peer)) = self.upstream_stats_peer_counters(upstream_name, peer_url)
        else {
            return;
        };
        counters.peer_timeouts_total.fetch_add(1, Ordering::Relaxed);
        peer.timeouts_total.fetch_add(1, Ordering::Relaxed);
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_failover(&self, upstream_name: &str) {
        let Some(counters) = self.upstream_stats_counters(upstream_name) else {
            return;
        };
        counters.failovers_total.fetch_add(1, Ordering::Relaxed);
        counters.recent_60s.failovers_total.increment_now();
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_completed_response(&self, upstream_name: &str) {
        let Some(counters) = self.upstream_stats_counters(upstream_name) else {
            return;
        };
        counters.completed_responses_total.fetch_add(1, Ordering::Relaxed);
        counters.recent_60s.completed_responses_total.increment_now();
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_bad_gateway_response(&self, upstream_name: &str) {
        let Some(counters) = self.upstream_stats_counters(upstream_name) else {
            return;
        };
        counters.bad_gateway_responses_total.fetch_add(1, Ordering::Relaxed);
        counters.recent_60s.bad_gateway_responses_total.increment_now();
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_gateway_timeout_response(&self, upstream_name: &str) {
        let Some(counters) = self.upstream_stats_counters(upstream_name) else {
            return;
        };
        counters.gateway_timeout_responses_total.fetch_add(1, Ordering::Relaxed);
        counters.recent_60s.gateway_timeout_responses_total.increment_now();
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_bad_request_response(&self, upstream_name: &str) {
        let Some(counters) = self.upstream_stats_counters(upstream_name) else {
            return;
        };
        counters.bad_request_responses_total.fetch_add(1, Ordering::Relaxed);
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_payload_too_large_response(&self, upstream_name: &str) {
        let Some(counters) = self.upstream_stats_counters(upstream_name) else {
            return;
        };
        counters.payload_too_large_responses_total.fetch_add(1, Ordering::Relaxed);
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_unsupported_media_type_response(&self, upstream_name: &str) {
        let Some(counters) = self.upstream_stats_counters(upstream_name) else {
            return;
        };
        counters.unsupported_media_type_responses_total.fetch_add(1, Ordering::Relaxed);
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_no_healthy_peers(&self, upstream_name: &str) {
        let Some(counters) = self.upstream_stats_counters(upstream_name) else {
            return;
        };
        counters.no_healthy_peers_total.fetch_add(1, Ordering::Relaxed);
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_peer_failure_class(
        &self,
        upstream_name: &str,
        failure_class: &str,
    ) {
        let Some(counters) = self.upstream_stats_counters(upstream_name) else {
            return;
        };
        match failure_class {
            "unknown_ca" => {
                counters.tls_failures_unknown_ca_total.fetch_add(1, Ordering::Relaxed);
            }
            "bad_certificate" => {
                counters.tls_failures_bad_certificate_total.fetch_add(1, Ordering::Relaxed);
            }
            "certificate_revoked" => {
                counters.tls_failures_certificate_revoked_total.fetch_add(1, Ordering::Relaxed);
            }
            "verify_depth_exceeded" => {
                counters.tls_failures_verify_depth_exceeded_total.fetch_add(1, Ordering::Relaxed);
            }
            _ => return,
        }
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
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

    fn upstream_stats_counters(&self, upstream_name: &str) -> Option<Arc<UpstreamStats>> {
        let stats = self.upstream_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        stats.get(upstream_name).map(|entry| entry.counters.clone())
    }

    fn upstream_stats_peer_counters(
        &self,
        upstream_name: &str,
        peer_url: &str,
    ) -> Option<(Arc<UpstreamStats>, Arc<UpstreamPeerStats>)> {
        let stats = self.upstream_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = stats.get(upstream_name)?;
        let peer = entry.peers.get(peer_url)?.clone();
        Some((entry.counters.clone(), peer))
    }
}
