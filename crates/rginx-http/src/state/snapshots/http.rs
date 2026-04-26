use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpCountersSnapshot {
    pub downstream_connections_accepted: u64,
    pub downstream_connections_rejected: u64,
    pub downstream_requests: u64,
    pub downstream_responses: u64,
    pub downstream_responses_1xx: u64,
    pub downstream_responses_2xx: u64,
    pub downstream_responses_3xx: u64,
    pub downstream_responses_4xx: u64,
    pub downstream_responses_5xx: u64,
    pub downstream_mtls_authenticated_connections: u64,
    pub downstream_mtls_authenticated_requests: u64,
    pub downstream_mtls_anonymous_requests: u64,
    pub downstream_tls_handshake_failures: u64,
    pub downstream_tls_handshake_failures_missing_client_cert: u64,
    pub downstream_tls_handshake_failures_unknown_ca: u64,
    pub downstream_tls_handshake_failures_bad_certificate: u64,
    pub downstream_tls_handshake_failures_certificate_revoked: u64,
    pub downstream_tls_handshake_failures_verify_depth_exceeded: u64,
    pub downstream_tls_handshake_failures_other: u64,
    pub downstream_http3_early_data_accepted_requests: u64,
    pub downstream_http3_early_data_rejected_requests: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MtlsStatusSnapshot {
    pub configured_listeners: usize,
    pub optional_listeners: usize,
    pub required_listeners: usize,
    pub authenticated_connections: u64,
    pub authenticated_requests: u64,
    pub anonymous_requests: u64,
    pub handshake_failures_total: u64,
    pub handshake_failures_missing_client_cert: u64,
    pub handshake_failures_unknown_ca: u64,
    pub handshake_failures_bad_certificate: u64,
    pub handshake_failures_certificate_revoked: u64,
    pub handshake_failures_verify_depth_exceeded: u64,
    pub handshake_failures_other: u64,
}
