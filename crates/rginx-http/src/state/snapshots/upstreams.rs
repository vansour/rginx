use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamTlsStatusSnapshot {
    pub upstream_name: String,
    pub protocol: String,
    pub verify_mode: String,
    pub tls_versions: Option<Vec<String>>,
    pub server_name_enabled: bool,
    pub server_name_override: Option<String>,
    pub verify_depth: Option<u32>,
    pub crl_configured: bool,
    pub client_identity_configured: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamPeerStatsSnapshot {
    pub peer_url: String,
    pub attempts_total: u64,
    pub successes_total: u64,
    pub failures_total: u64,
    pub timeouts_total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamStatsSnapshot {
    pub upstream_name: String,
    pub tls: UpstreamTlsStatusSnapshot,
    pub downstream_requests_total: u64,
    pub peer_attempts_total: u64,
    pub peer_successes_total: u64,
    pub peer_failures_total: u64,
    pub peer_timeouts_total: u64,
    pub failovers_total: u64,
    pub completed_responses_total: u64,
    pub bad_gateway_responses_total: u64,
    pub gateway_timeout_responses_total: u64,
    pub bad_request_responses_total: u64,
    pub payload_too_large_responses_total: u64,
    pub unsupported_media_type_responses_total: u64,
    pub no_healthy_peers_total: u64,
    pub tls_failures_unknown_ca_total: u64,
    pub tls_failures_bad_certificate_total: u64,
    pub tls_failures_certificate_revoked_total: u64,
    pub tls_failures_verify_depth_exceeded_total: u64,
    pub recent_60s: RecentUpstreamStatsSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_window: Option<RecentUpstreamStatsSnapshot>,
    pub peers: Vec<UpstreamPeerStatsSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RecentUpstreamStatsSnapshot {
    pub window_secs: u64,
    pub downstream_requests_total: u64,
    pub peer_attempts_total: u64,
    pub completed_responses_total: u64,
    pub bad_gateway_responses_total: u64,
    pub gateway_timeout_responses_total: u64,
    pub failovers_total: u64,
}
