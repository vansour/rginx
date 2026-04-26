#[derive(Debug, Default)]
struct HttpCounters {
    downstream_connections_accepted: AtomicU64,
    downstream_connections_rejected: AtomicU64,
    downstream_requests: AtomicU64,
    downstream_responses: AtomicU64,
    downstream_responses_1xx: AtomicU64,
    downstream_responses_2xx: AtomicU64,
    downstream_responses_3xx: AtomicU64,
    downstream_responses_4xx: AtomicU64,
    downstream_responses_5xx: AtomicU64,
    downstream_mtls_authenticated_connections: AtomicU64,
    downstream_mtls_authenticated_requests: AtomicU64,
    downstream_mtls_anonymous_requests: AtomicU64,
    downstream_tls_handshake_failures: AtomicU64,
    downstream_tls_handshake_failures_missing_client_cert: AtomicU64,
    downstream_tls_handshake_failures_unknown_ca: AtomicU64,
    downstream_tls_handshake_failures_bad_certificate: AtomicU64,
    downstream_tls_handshake_failures_certificate_revoked: AtomicU64,
    downstream_tls_handshake_failures_verify_depth_exceeded: AtomicU64,
    downstream_tls_handshake_failures_other: AtomicU64,
    downstream_http3_early_data_accepted_requests: AtomicU64,
    downstream_http3_early_data_rejected_requests: AtomicU64,
}

#[derive(Debug, Default)]
struct ReloadHistory {
    attempts_total: u64,
    successes_total: u64,
    failures_total: u64,
    last_result: Option<ReloadResultSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TlsHandshakeFailureReason {
    MissingClientCert,
    UnknownCa,
    BadCertificate,
    CertificateRevoked,
    VerifyDepthExceeded,
    Other,
}

impl TlsHandshakeFailureReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::MissingClientCert => "missing_client_cert",
            Self::UnknownCa => "unknown_ca",
            Self::BadCertificate => "bad_certificate",
            Self::CertificateRevoked => "certificate_revoked",
            Self::VerifyDepthExceeded => "verify_depth_exceeded",
            Self::Other => "other",
        }
    }
}

impl HttpCounters {
    fn snapshot(&self) -> HttpCountersSnapshot {
        HttpCountersSnapshot {
            downstream_connections_accepted: self
                .downstream_connections_accepted
                .load(Ordering::Relaxed),
            downstream_connections_rejected: self
                .downstream_connections_rejected
                .load(Ordering::Relaxed),
            downstream_requests: self.downstream_requests.load(Ordering::Relaxed),
            downstream_responses: self.downstream_responses.load(Ordering::Relaxed),
            downstream_responses_1xx: self.downstream_responses_1xx.load(Ordering::Relaxed),
            downstream_responses_2xx: self.downstream_responses_2xx.load(Ordering::Relaxed),
            downstream_responses_3xx: self.downstream_responses_3xx.load(Ordering::Relaxed),
            downstream_responses_4xx: self.downstream_responses_4xx.load(Ordering::Relaxed),
            downstream_responses_5xx: self.downstream_responses_5xx.load(Ordering::Relaxed),
            downstream_mtls_authenticated_connections: self
                .downstream_mtls_authenticated_connections
                .load(Ordering::Relaxed),
            downstream_mtls_authenticated_requests: self
                .downstream_mtls_authenticated_requests
                .load(Ordering::Relaxed),
            downstream_mtls_anonymous_requests: self
                .downstream_mtls_anonymous_requests
                .load(Ordering::Relaxed),
            downstream_tls_handshake_failures: self
                .downstream_tls_handshake_failures
                .load(Ordering::Relaxed),
            downstream_tls_handshake_failures_missing_client_cert: self
                .downstream_tls_handshake_failures_missing_client_cert
                .load(Ordering::Relaxed),
            downstream_tls_handshake_failures_unknown_ca: self
                .downstream_tls_handshake_failures_unknown_ca
                .load(Ordering::Relaxed),
            downstream_tls_handshake_failures_bad_certificate: self
                .downstream_tls_handshake_failures_bad_certificate
                .load(Ordering::Relaxed),
            downstream_tls_handshake_failures_certificate_revoked: self
                .downstream_tls_handshake_failures_certificate_revoked
                .load(Ordering::Relaxed),
            downstream_tls_handshake_failures_verify_depth_exceeded: self
                .downstream_tls_handshake_failures_verify_depth_exceeded
                .load(Ordering::Relaxed),
            downstream_tls_handshake_failures_other: self
                .downstream_tls_handshake_failures_other
                .load(Ordering::Relaxed),
            downstream_http3_early_data_accepted_requests: self
                .downstream_http3_early_data_accepted_requests
                .load(Ordering::Relaxed),
            downstream_http3_early_data_rejected_requests: self
                .downstream_http3_early_data_rejected_requests
                .load(Ordering::Relaxed),
        }
    }
}

impl ReloadHistory {
    fn snapshot(&self) -> ReloadStatusSnapshot {
        ReloadStatusSnapshot {
            attempts_total: self.attempts_total,
            successes_total: self.successes_total,
            failures_total: self.failures_total,
            last_result: self.last_result.clone(),
        }
    }
}
