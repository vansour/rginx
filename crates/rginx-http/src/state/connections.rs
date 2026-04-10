use super::*;

pub struct ActiveConnectionGuard {
    pub(super) active_connections: Arc<AtomicUsize>,
    pub(super) listener_active_connections: Arc<AtomicUsize>,
    pub(super) listener_id: String,
    pub(super) snapshot_version: Arc<AtomicU64>,
    pub(super) snapshot_notify: Arc<Notify>,
    pub(super) snapshot_components: Arc<SnapshotComponentVersions>,
    pub(super) traffic_component_versions: Arc<StdRwLock<TrafficComponentVersions>>,
}

impl Drop for ActiveConnectionGuard {
    fn drop(&mut self) {
        self.active_connections.fetch_sub(1, Ordering::AcqRel);
        self.listener_active_connections.fetch_sub(1, Ordering::AcqRel);
        let version = self.snapshot_version.fetch_add(1, Ordering::Relaxed) + 1;
        self.snapshot_components.status.store(version, Ordering::Relaxed);
        self.snapshot_components.traffic.store(version, Ordering::Relaxed);
        self.traffic_component_versions
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .listeners
            .insert(self.listener_id.clone(), version);
        self.snapshot_notify.notify_waiters();
    }
}

impl SharedState {
    pub fn active_connection_count(&self) -> usize {
        self.active_connections.load(Ordering::Acquire)
    }

    pub fn try_acquire_connection(
        &self,
        listener_id: &str,
        limit: Option<usize>,
    ) -> Option<ActiveConnectionGuard> {
        let listener_active_connections = self
            .listener_active_connections
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(listener_id)?
            .clone();
        loop {
            let current = listener_active_connections.load(Ordering::Acquire);
            if limit.is_some_and(|limit| current >= limit) {
                return None;
            }

            if listener_active_connections
                .compare_exchange_weak(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                self.active_connections.fetch_add(1, Ordering::AcqRel);
                return Some(ActiveConnectionGuard {
                    active_connections: self.active_connections.clone(),
                    listener_active_connections,
                    listener_id: listener_id.to_string(),
                    snapshot_version: self.snapshot_version.clone(),
                    snapshot_notify: self.snapshot_notify.clone(),
                    snapshot_components: self.snapshot_components.clone(),
                    traffic_component_versions: self.traffic_component_versions.clone(),
                });
            }
        }
    }

    pub fn retain_connection_slot(&self, listener_id: &str) -> ActiveConnectionGuard {
        let listener_active_connections = self
            .listener_active_connections
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(listener_id)
            .expect("listener id should exist while retaining a connection slot")
            .clone();
        listener_active_connections.fetch_add(1, Ordering::AcqRel);
        self.active_connections.fetch_add(1, Ordering::AcqRel);
        let version = self.mark_snapshot_changed_components(true, false, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
        ActiveConnectionGuard {
            active_connections: self.active_connections.clone(),
            listener_active_connections,
            listener_id: listener_id.to_string(),
            snapshot_version: self.snapshot_version.clone(),
            snapshot_notify: self.snapshot_notify.clone(),
            snapshot_components: self.snapshot_components.clone(),
            traffic_component_versions: self.traffic_component_versions.clone(),
        }
    }

    pub(crate) fn record_connection_accepted(&self, listener_id: &str) {
        self.counters.downstream_connections_accepted.fetch_add(1, Ordering::Relaxed);
        if let Some(counters) = self.listener_traffic_counters(listener_id) {
            counters.downstream_connections_accepted.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(true, true, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
    }

    pub(crate) fn record_mtls_handshake_success(&self, listener_id: &str, authenticated: bool) {
        if !authenticated {
            return;
        }

        self.counters.downstream_mtls_authenticated_connections.fetch_add(1, Ordering::Relaxed);
        if let Some(counters) = self.listener_traffic_counters(listener_id) {
            counters.downstream_mtls_authenticated_connections.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(true, true, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
    }

    pub(crate) fn record_tls_handshake_failure(
        &self,
        listener_id: &str,
        reason: TlsHandshakeFailureReason,
    ) {
        self.counters.downstream_tls_handshake_failures.fetch_add(1, Ordering::Relaxed);
        match reason {
            TlsHandshakeFailureReason::MissingClientCert => {
                self.counters
                    .downstream_tls_handshake_failures_missing_client_cert
                    .fetch_add(1, Ordering::Relaxed);
            }
            TlsHandshakeFailureReason::UnknownCa => {
                self.counters
                    .downstream_tls_handshake_failures_unknown_ca
                    .fetch_add(1, Ordering::Relaxed);
            }
            TlsHandshakeFailureReason::BadCertificate => {
                self.counters
                    .downstream_tls_handshake_failures_bad_certificate
                    .fetch_add(1, Ordering::Relaxed);
            }
            TlsHandshakeFailureReason::CertificateRevoked => {
                self.counters
                    .downstream_tls_handshake_failures_certificate_revoked
                    .fetch_add(1, Ordering::Relaxed);
            }
            TlsHandshakeFailureReason::VerifyDepthExceeded => {
                self.counters
                    .downstream_tls_handshake_failures_verify_depth_exceeded
                    .fetch_add(1, Ordering::Relaxed);
            }
            TlsHandshakeFailureReason::Other => {
                self.counters
                    .downstream_tls_handshake_failures_other
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        if let Some(counters) = self.listener_traffic_counters(listener_id) {
            counters.downstream_tls_handshake_failures.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(true, true, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
    }

    pub(crate) fn record_connection_rejected(&self, listener_id: &str) {
        self.counters.downstream_connections_rejected.fetch_add(1, Ordering::Relaxed);
        if let Some(counters) = self.listener_traffic_counters(listener_id) {
            counters.downstream_connections_rejected.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(true, true, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
    }
}
