use super::super::*;

impl SharedState {
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
        self.notify_snapshot_waiters();
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
        self.notify_snapshot_waiters();
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
        self.notify_snapshot_waiters();
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
        self.notify_snapshot_waiters();
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
        self.notify_snapshot_waiters();
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
        self.notify_snapshot_waiters();
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
        self.notify_snapshot_waiters();
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
        self.notify_snapshot_waiters();
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
        self.notify_snapshot_waiters();
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
        self.notify_snapshot_waiters();
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
        self.notify_snapshot_waiters();
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
        self.notify_snapshot_waiters();
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
        self.notify_snapshot_waiters();
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
        self.notify_snapshot_waiters();
    }
}
