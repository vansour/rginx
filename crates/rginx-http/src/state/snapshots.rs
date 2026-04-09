#[derive(Clone)]
pub struct ActiveState {
    pub revision: u64,
    pub config: Arc<ConfigSnapshot>,
    pub clients: ProxyClients,
}

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsReloadBoundarySnapshot {
    pub reloadable_fields: Vec<String>,
    pub restart_required_fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsListenerStatusSnapshot {
    pub listener_id: String,
    pub listener_name: String,
    pub listen_addr: std::net::SocketAddr,
    pub tls_enabled: bool,
    pub default_certificate: Option<String>,
    pub versions: Option<Vec<String>>,
    pub alpn_protocols: Vec<String>,
    pub session_resumption_enabled: Option<bool>,
    pub session_tickets_enabled: Option<bool>,
    pub session_cache_size: Option<usize>,
    pub session_ticket_count: Option<usize>,
    pub client_auth_mode: Option<String>,
    pub client_auth_verify_depth: Option<u32>,
    pub client_auth_crl_configured: bool,
    pub sni_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsCertificateStatusSnapshot {
    pub scope: String,
    pub cert_path: PathBuf,
    pub server_names: Vec<String>,
    pub subject: Option<String>,
    pub issuer: Option<String>,
    pub serial_number: Option<String>,
    pub san_dns_names: Vec<String>,
    pub fingerprint_sha256: Option<String>,
    pub subject_key_identifier: Option<String>,
    pub authority_key_identifier: Option<String>,
    pub is_ca: Option<bool>,
    pub path_len_constraint: Option<u32>,
    pub key_usage: Option<String>,
    pub extended_key_usage: Vec<String>,
    pub not_before_unix_ms: Option<u64>,
    pub not_after_unix_ms: Option<u64>,
    pub expires_in_days: Option<i64>,
    pub chain_length: usize,
    pub chain_subjects: Vec<String>,
    pub chain_diagnostics: Vec<String>,
    pub selected_as_default_for_listeners: Vec<String>,
    pub ocsp_staple_configured: bool,
    pub additional_certificate_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsOcspStatusSnapshot {
    pub scope: String,
    pub cert_path: PathBuf,
    pub ocsp_staple_path: Option<PathBuf>,
    pub responder_urls: Vec<String>,
    pub cache_loaded: bool,
    pub cache_size_bytes: Option<usize>,
    pub cache_modified_unix_ms: Option<u64>,
    pub auto_refresh_enabled: bool,
    pub last_refresh_unix_ms: Option<u64>,
    pub refreshes_total: u64,
    pub failures_total: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsVhostBindingSnapshot {
    pub listener_name: String,
    pub vhost_id: String,
    pub server_names: Vec<String>,
    pub certificate_scopes: Vec<String>,
    pub fingerprints: Vec<String>,
    pub default_selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsSniBindingSnapshot {
    pub listener_name: String,
    pub server_name: String,
    pub certificate_scopes: Vec<String>,
    pub fingerprints: Vec<String>,
    pub default_selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsDefaultCertificateBindingSnapshot {
    pub listener_name: String,
    pub server_name: String,
    pub certificate_scopes: Vec<String>,
    pub fingerprints: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsRuntimeSnapshot {
    pub listeners: Vec<TlsListenerStatusSnapshot>,
    pub certificates: Vec<TlsCertificateStatusSnapshot>,
    pub ocsp: Vec<TlsOcspStatusSnapshot>,
    pub vhost_bindings: Vec<TlsVhostBindingSnapshot>,
    pub sni_bindings: Vec<TlsSniBindingSnapshot>,
    pub sni_conflicts: Vec<TlsSniBindingSnapshot>,
    pub default_certificate_bindings: Vec<TlsDefaultCertificateBindingSnapshot>,
    pub reload_boundary: TlsReloadBoundarySnapshot,
    pub expiring_certificate_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReloadOutcomeSnapshot {
    Success { revision: u64 },
    Failure { error: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReloadResultSnapshot {
    pub finished_at_unix_ms: u64,
    pub outcome: ReloadOutcomeSnapshot,
    pub tls_certificate_changes: Vec<String>,
    pub active_revision: u64,
    pub rollback_preserved_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReloadStatusSnapshot {
    pub attempts_total: u64,
    pub successes_total: u64,
    pub failures_total: u64,
    pub last_result: Option<ReloadResultSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotDeltaSnapshot {
    pub schema_version: u32,
    pub since_version: u64,
    pub current_snapshot_version: u64,
    pub included_modules: Vec<SnapshotModule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_window_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counters_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traffic_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_health_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstreams_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_changed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counters_changed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traffic_changed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traffic_recent_changed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_health_changed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstreams_changed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstreams_recent_changed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_listener_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_vhost_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_route_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_recent_listener_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_recent_vhost_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_recent_route_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_peer_health_upstream_names: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_upstream_names: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_recent_upstream_names: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotModule {
    Status,
    Counters,
    Traffic,
    PeerHealth,
    Upstreams,
}

impl SnapshotModule {
    pub const ALL: [Self; 5] =
        [Self::Status, Self::Counters, Self::Traffic, Self::PeerHealth, Self::Upstreams];

    pub fn all() -> Vec<Self> {
        Self::ALL.to_vec()
    }

    pub fn normalize(include: Option<&[Self]>) -> Vec<Self> {
        let requested = include.unwrap_or(&Self::ALL);
        Self::ALL.iter().copied().filter(|module| requested.contains(module)).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeStatusSnapshot {
    pub revision: u64,
    pub config_path: Option<PathBuf>,
    pub listen_addr: std::net::SocketAddr,
    pub worker_threads: Option<usize>,
    pub accept_workers: usize,
    pub total_vhosts: usize,
    pub total_routes: usize,
    pub total_upstreams: usize,
    pub tls_enabled: bool,
    pub tls: TlsRuntimeSnapshot,
    pub mtls: MtlsStatusSnapshot,
    pub upstream_tls: Vec<UpstreamTlsStatusSnapshot>,
    pub active_connections: usize,
    pub reload: ReloadStatusSnapshot,
}

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

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GrpcTrafficSnapshot {
    pub requests_total: u64,
    pub protocol_grpc_total: u64,
    pub protocol_grpc_web_total: u64,
    pub protocol_grpc_web_text_total: u64,
    pub status_0_total: u64,
    pub status_1_total: u64,
    pub status_3_total: u64,
    pub status_4_total: u64,
    pub status_7_total: u64,
    pub status_8_total: u64,
    pub status_12_total: u64,
    pub status_14_total: u64,
    pub status_other_total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenerStatsSnapshot {
    pub listener_id: String,
    pub listener_name: String,
    pub listen_addr: std::net::SocketAddr,
    pub active_connections: usize,
    pub downstream_connections_accepted: u64,
    pub downstream_connections_rejected: u64,
    pub downstream_requests: u64,
    pub unmatched_requests_total: u64,
    pub downstream_responses: u64,
    pub downstream_responses_1xx: u64,
    pub downstream_responses_2xx: u64,
    pub downstream_responses_3xx: u64,
    pub downstream_responses_4xx: u64,
    pub downstream_responses_5xx: u64,
    pub recent_60s: RecentTrafficStatsSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_window: Option<RecentTrafficStatsSnapshot>,
    pub grpc: GrpcTrafficSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VhostStatsSnapshot {
    pub vhost_id: String,
    pub server_names: Vec<String>,
    pub downstream_requests: u64,
    pub unmatched_requests_total: u64,
    pub downstream_responses: u64,
    pub downstream_responses_1xx: u64,
    pub downstream_responses_2xx: u64,
    pub downstream_responses_3xx: u64,
    pub downstream_responses_4xx: u64,
    pub downstream_responses_5xx: u64,
    pub recent_60s: RecentTrafficStatsSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_window: Option<RecentTrafficStatsSnapshot>,
    pub grpc: GrpcTrafficSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteStatsSnapshot {
    pub route_id: String,
    pub vhost_id: String,
    pub downstream_requests: u64,
    pub downstream_responses: u64,
    pub downstream_responses_1xx: u64,
    pub downstream_responses_2xx: u64,
    pub downstream_responses_3xx: u64,
    pub downstream_responses_4xx: u64,
    pub downstream_responses_5xx: u64,
    pub access_denied_total: u64,
    pub rate_limited_total: u64,
    pub recent_60s: RecentTrafficStatsSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_window: Option<RecentTrafficStatsSnapshot>,
    pub grpc: GrpcTrafficSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RecentTrafficStatsSnapshot {
    pub window_secs: u64,
    pub downstream_requests_total: u64,
    pub downstream_responses_total: u64,
    pub downstream_responses_2xx_total: u64,
    pub downstream_responses_4xx_total: u64,
    pub downstream_responses_5xx_total: u64,
    pub grpc_requests_total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficStatsSnapshot {
    pub listeners: Vec<ListenerStatsSnapshot>,
    pub vhosts: Vec<VhostStatsSnapshot>,
    pub routes: Vec<RouteStatsSnapshot>,
}
