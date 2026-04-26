use super::super::super::*;

impl SharedState {
    pub(crate) fn record_connection_accepted(&self, listener_id: &str) {
        self.counters.downstream_connections_accepted.fetch_add(1, Ordering::Relaxed);
        if let Some(counters) = self.listener_traffic_counters(listener_id) {
            counters.downstream_connections_accepted.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(true, true, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
        self.notify_snapshot_waiters();
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
        self.notify_snapshot_waiters();
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
                if let Some(counters) = self.listener_traffic_counters(listener_id) {
                    counters
                        .downstream_tls_handshake_failures_missing_client_cert
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            TlsHandshakeFailureReason::UnknownCa => {
                self.counters
                    .downstream_tls_handshake_failures_unknown_ca
                    .fetch_add(1, Ordering::Relaxed);
                if let Some(counters) = self.listener_traffic_counters(listener_id) {
                    counters
                        .downstream_tls_handshake_failures_unknown_ca
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            TlsHandshakeFailureReason::BadCertificate => {
                self.counters
                    .downstream_tls_handshake_failures_bad_certificate
                    .fetch_add(1, Ordering::Relaxed);
                if let Some(counters) = self.listener_traffic_counters(listener_id) {
                    counters
                        .downstream_tls_handshake_failures_bad_certificate
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            TlsHandshakeFailureReason::CertificateRevoked => {
                self.counters
                    .downstream_tls_handshake_failures_certificate_revoked
                    .fetch_add(1, Ordering::Relaxed);
                if let Some(counters) = self.listener_traffic_counters(listener_id) {
                    counters
                        .downstream_tls_handshake_failures_certificate_revoked
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            TlsHandshakeFailureReason::VerifyDepthExceeded => {
                self.counters
                    .downstream_tls_handshake_failures_verify_depth_exceeded
                    .fetch_add(1, Ordering::Relaxed);
                if let Some(counters) = self.listener_traffic_counters(listener_id) {
                    counters
                        .downstream_tls_handshake_failures_verify_depth_exceeded
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            TlsHandshakeFailureReason::Other => {
                self.counters
                    .downstream_tls_handshake_failures_other
                    .fetch_add(1, Ordering::Relaxed);
                if let Some(counters) = self.listener_traffic_counters(listener_id) {
                    counters
                        .downstream_tls_handshake_failures_other
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        if let Some(counters) = self.listener_traffic_counters(listener_id) {
            counters.downstream_tls_handshake_failures.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(true, true, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
        self.notify_snapshot_waiters();
    }

    pub(crate) fn record_connection_rejected(&self, listener_id: &str) {
        self.counters.downstream_connections_rejected.fetch_add(1, Ordering::Relaxed);
        if let Some(counters) = self.listener_traffic_counters(listener_id) {
            counters.downstream_connections_rejected.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(true, true, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
        self.notify_snapshot_waiters();
    }

    pub(crate) fn record_http3_early_data_accepted_request(&self, listener_id: &str) {
        self.counters.downstream_http3_early_data_accepted_requests.fetch_add(1, Ordering::Relaxed);
        if let Some(counters) = self.listener_traffic_counters(listener_id) {
            counters.downstream_http3_early_data_accepted_requests.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(true, true, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
        self.notify_snapshot_waiters();
    }

    pub(crate) fn record_http3_early_data_rejected_request(&self, listener_id: &str) {
        self.counters.downstream_http3_early_data_rejected_requests.fetch_add(1, Ordering::Relaxed);
        if let Some(counters) = self.listener_traffic_counters(listener_id) {
            counters.downstream_http3_early_data_rejected_requests.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(true, true, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
        self.notify_snapshot_waiters();
    }

    pub(crate) fn record_http3_retry_issued(&self, listener_id: &str) {
        if let Some(counters) = self.listener_traffic_counters(listener_id) {
            counters.http3_retry_issued_total.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(true, false, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
        self.notify_snapshot_waiters();
    }

    pub(crate) fn record_http3_retry_failed(&self, listener_id: &str) {
        if let Some(counters) = self.listener_traffic_counters(listener_id) {
            counters.http3_retry_failed_total.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(true, false, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
        self.notify_snapshot_waiters();
    }

    pub(crate) fn record_http3_request_accept_error(&self, listener_id: &str) {
        if let Some(counters) = self.listener_traffic_counters(listener_id) {
            counters.http3_request_accept_errors_total.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(true, false, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
        self.notify_snapshot_waiters();
    }

    pub(crate) fn record_http3_request_resolve_error(&self, listener_id: &str) {
        if let Some(counters) = self.listener_traffic_counters(listener_id) {
            counters.http3_request_resolve_errors_total.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(true, false, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
        self.notify_snapshot_waiters();
    }

    pub(crate) fn record_http3_request_body_stream_error(&self, listener_id: &str) {
        if let Some(counters) = self.listener_traffic_counters(listener_id) {
            counters.http3_request_body_stream_errors_total.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(true, false, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
        self.notify_snapshot_waiters();
    }

    pub(crate) fn record_http3_response_stream_error(&self, listener_id: &str) {
        if let Some(counters) = self.listener_traffic_counters(listener_id) {
            counters.http3_response_stream_errors_total.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(true, false, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
        self.notify_snapshot_waiters();
    }

    pub(crate) fn record_http3_connection_close(
        &self,
        listener_id: &str,
        reason: quinn::ConnectionError,
    ) {
        if let Some(counters) = self.listener_traffic_counters(listener_id) {
            match reason {
                quinn::ConnectionError::VersionMismatch => {
                    counters
                        .http3_connection_close_version_mismatch_total
                        .fetch_add(1, Ordering::Relaxed);
                }
                quinn::ConnectionError::TransportError(_) => {
                    counters
                        .http3_connection_close_transport_error_total
                        .fetch_add(1, Ordering::Relaxed);
                }
                quinn::ConnectionError::ConnectionClosed(_) => {
                    counters
                        .http3_connection_close_connection_closed_total
                        .fetch_add(1, Ordering::Relaxed);
                }
                quinn::ConnectionError::ApplicationClosed(_) => {
                    counters
                        .http3_connection_close_application_closed_total
                        .fetch_add(1, Ordering::Relaxed);
                }
                quinn::ConnectionError::Reset => {
                    counters.http3_connection_close_reset_total.fetch_add(1, Ordering::Relaxed);
                }
                quinn::ConnectionError::TimedOut => {
                    counters.http3_connection_close_timed_out_total.fetch_add(1, Ordering::Relaxed);
                }
                quinn::ConnectionError::LocallyClosed => {
                    counters
                        .http3_connection_close_locally_closed_total
                        .fetch_add(1, Ordering::Relaxed);
                }
                quinn::ConnectionError::CidsExhausted => {
                    counters
                        .http3_connection_close_cids_exhausted_total
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        let version = self.mark_snapshot_changed_components(true, false, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
        self.notify_snapshot_waiters();
    }
}
