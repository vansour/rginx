use super::super::*;

impl SharedState {
    pub(super) fn mtls_status_snapshot(&self, config: &ConfigSnapshot) -> MtlsStatusSnapshot {
        let mut configured_listeners = 0usize;
        let mut optional_listeners = 0usize;
        let mut required_listeners = 0usize;
        let mut authenticated_connections = 0u64;
        let mut authenticated_requests = 0u64;
        let mut anonymous_requests = 0u64;
        let mut handshake_failures_total = 0u64;
        let mut handshake_failures_missing_client_cert = 0u64;
        let mut handshake_failures_unknown_ca = 0u64;
        let mut handshake_failures_bad_certificate = 0u64;
        let mut handshake_failures_certificate_revoked = 0u64;
        let mut handshake_failures_verify_depth_exceeded = 0u64;
        let mut handshake_failures_other = 0u64;

        for listener in &config.listeners {
            let Some(client_auth) =
                listener.server.tls.as_ref().and_then(|tls| tls.client_auth.as_ref())
            else {
                continue;
            };
            configured_listeners += 1;
            match client_auth.mode {
                rginx_core::ServerClientAuthMode::Optional => optional_listeners += 1,
                rginx_core::ServerClientAuthMode::Required => required_listeners += 1,
            }

            if let Some(counters) = self.listener_traffic_counters(&listener.id) {
                authenticated_connections +=
                    counters.downstream_mtls_authenticated_connections.load(Ordering::Relaxed);
                authenticated_requests +=
                    counters.downstream_mtls_authenticated_requests.load(Ordering::Relaxed);
                anonymous_requests +=
                    counters.downstream_mtls_anonymous_requests.load(Ordering::Relaxed);
                handshake_failures_total +=
                    counters.downstream_tls_handshake_failures.load(Ordering::Relaxed);
                handshake_failures_missing_client_cert += counters
                    .downstream_tls_handshake_failures_missing_client_cert
                    .load(Ordering::Relaxed);
                handshake_failures_unknown_ca +=
                    counters.downstream_tls_handshake_failures_unknown_ca.load(Ordering::Relaxed);
                handshake_failures_bad_certificate += counters
                    .downstream_tls_handshake_failures_bad_certificate
                    .load(Ordering::Relaxed);
                handshake_failures_certificate_revoked += counters
                    .downstream_tls_handshake_failures_certificate_revoked
                    .load(Ordering::Relaxed);
                handshake_failures_verify_depth_exceeded += counters
                    .downstream_tls_handshake_failures_verify_depth_exceeded
                    .load(Ordering::Relaxed);
                handshake_failures_other +=
                    counters.downstream_tls_handshake_failures_other.load(Ordering::Relaxed);
            }
        }

        MtlsStatusSnapshot {
            configured_listeners,
            optional_listeners,
            required_listeners,
            authenticated_connections,
            authenticated_requests,
            anonymous_requests,
            handshake_failures_total,
            handshake_failures_missing_client_cert,
            handshake_failures_unknown_ca,
            handshake_failures_bad_certificate,
            handshake_failures_certificate_revoked,
            handshake_failures_verify_depth_exceeded,
            handshake_failures_other,
        }
    }
}
