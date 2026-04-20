use serde::{Deserialize, Serialize};

use rginx_control_types::NodeSnapshotDetail;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RuntimeStatusSnapshot {
    pub revision: Option<u64>,
    pub config_path: Option<String>,
    pub listeners: Vec<RuntimeListenerSnapshot>,
    pub worker_threads: Option<u32>,
    pub accept_workers: u32,
    pub total_vhosts: u32,
    pub total_routes: u32,
    pub total_upstreams: u32,
    pub tls_enabled: bool,
    pub http3_active_connections: u64,
    pub http3_active_request_streams: u64,
    pub http3_early_data_enabled_listeners: u32,
    pub upstream_tls: Vec<UpstreamTlsStatusSnapshot>,
    pub tls: TlsRuntimeSnapshot,
    pub mtls: MtlsStatusSnapshot,
    pub active_connections: u64,
    pub reload: ReloadStatusSnapshot,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RuntimeListenerSnapshot {
    pub listener_id: String,
    pub listener_name: String,
    pub listen_addr: String,
    pub binding_count: u32,
    pub http3_enabled: bool,
    pub tls_enabled: bool,
    pub proxy_protocol_enabled: bool,
    pub default_certificate: Option<String>,
    pub keep_alive: bool,
    pub max_connections: Option<u64>,
    pub access_log_format_configured: bool,
    pub bindings: Vec<RuntimeListenerBindingSnapshot>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RuntimeListenerBindingSnapshot {
    pub binding_name: String,
    pub transport: String,
    pub listen_addr: String,
    pub protocols: Vec<String>,
    pub worker_count: u32,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ReloadStatusSnapshot {
    pub attempts_total: u64,
    pub successes_total: u64,
    pub failures_total: u64,
    pub last_result: Option<ReloadResultSnapshot>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ReloadResultSnapshot {
    pub finished_at_unix_ms: u64,
    pub outcome: serde_json::Value,
    pub tls_certificate_changes: Vec<String>,
    pub active_revision: u64,
    pub rollback_preserved_revision: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct HttpCountersSnapshot {
    pub downstream_connections_accepted: u64,
    pub downstream_connections_rejected: u64,
    pub downstream_requests: u64,
    pub downstream_responses: u64,
    pub downstream_responses_2xx: u64,
    pub downstream_responses_4xx: u64,
    pub downstream_responses_5xx: u64,
    pub downstream_mtls_authenticated_requests: u64,
    pub downstream_tls_handshake_failures: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct TrafficStatsSnapshot {
    pub listeners: Vec<ListenerStatsSnapshot>,
    pub vhosts: Vec<VhostStatsSnapshot>,
    pub routes: Vec<RouteStatsSnapshot>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ListenerStatsSnapshot {
    pub listener_id: String,
    pub listener_name: String,
    pub listen_addr: String,
    pub active_connections: u64,
    pub downstream_connections_accepted: u64,
    pub unmatched_requests_total: u64,
    pub downstream_requests: u64,
    pub downstream_responses: u64,
    pub recent_60s: RecentTrafficStatsSnapshot,
    pub grpc: GrpcTrafficSnapshot,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct VhostStatsSnapshot {
    pub vhost_id: String,
    pub server_names: Vec<String>,
    pub downstream_requests: u64,
    pub unmatched_requests_total: u64,
    pub downstream_responses: u64,
    pub recent_60s: RecentTrafficStatsSnapshot,
    pub grpc: GrpcTrafficSnapshot,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RouteStatsSnapshot {
    pub route_id: String,
    pub vhost_id: String,
    pub downstream_requests: u64,
    pub downstream_responses: u64,
    pub access_denied_total: u64,
    pub rate_limited_total: u64,
    pub recent_60s: RecentTrafficStatsSnapshot,
    pub grpc: GrpcTrafficSnapshot,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RecentTrafficStatsSnapshot {
    pub window_secs: u64,
    pub downstream_requests_total: u64,
    pub downstream_responses_total: u64,
    pub downstream_responses_2xx_total: u64,
    pub downstream_responses_4xx_total: u64,
    pub downstream_responses_5xx_total: u64,
    pub grpc_requests_total: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct GrpcTrafficSnapshot {
    pub requests_total: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct UpstreamHealthSnapshot {
    pub upstream_name: String,
    pub unhealthy_after_failures: u64,
    pub cooldown_ms: u64,
    pub active_health_enabled: bool,
    pub resolver: UpstreamResolverRuntimeSnapshot,
    pub peers: Vec<PeerHealthSnapshot>,
    pub endpoints: Vec<ResolvedEndpointHealthSnapshot>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct PeerHealthSnapshot {
    pub peer_url: String,
    pub backup: bool,
    pub weight: u32,
    pub available: bool,
    pub passive_consecutive_failures: u64,
    pub passive_cooldown_remaining_ms: Option<u64>,
    pub passive_pending_recovery: bool,
    pub active_unhealthy: bool,
    pub active_consecutive_successes: u64,
    pub active_requests: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ResolvedEndpointHealthSnapshot {
    pub endpoint_key: String,
    pub logical_peer_url: String,
    pub display_url: String,
    pub dial_addr: String,
    pub server_name: String,
    pub backup: bool,
    pub weight: u32,
    pub available: bool,
    pub passive_consecutive_failures: u64,
    pub passive_cooldown_remaining_ms: Option<u64>,
    pub passive_pending_recovery: bool,
    pub active_unhealthy: bool,
    pub active_consecutive_successes: u64,
    pub active_requests: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct UpstreamResolverRuntimeSnapshot {
    pub resolve_requests_total: u64,
    pub cache_hits_total: u64,
    pub cache_misses_total: u64,
    pub refreshes_total: u64,
    pub resolve_errors_total: u64,
    pub stale_answers_total: u64,
    pub cache_entries: Vec<UpstreamResolverCacheEntrySnapshot>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct UpstreamResolverCacheEntrySnapshot {
    pub hostname: String,
    pub addresses: Vec<String>,
    pub negative: bool,
    pub valid_for_ms: Option<u64>,
    pub stale_for_ms: Option<u64>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct UpstreamStatsSnapshot {
    pub upstream_name: String,
    pub downstream_requests_total: u64,
    pub peer_attempts_total: u64,
    pub peer_successes_total: u64,
    pub peer_failures_total: u64,
    pub peer_timeouts_total: u64,
    pub failovers_total: u64,
    pub completed_responses_total: u64,
    pub bad_gateway_responses_total: u64,
    pub gateway_timeout_responses_total: u64,
    pub no_healthy_peers_total: u64,
    pub recent_60s: RecentUpstreamStatsSnapshot,
    pub tls: UpstreamTlsStatusSnapshot,
    pub peers: Vec<UpstreamPeerStatsSnapshot>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RecentUpstreamStatsSnapshot {
    pub window_secs: u64,
    pub downstream_requests_total: u64,
    pub peer_attempts_total: u64,
    pub completed_responses_total: u64,
    pub bad_gateway_responses_total: u64,
    pub gateway_timeout_responses_total: u64,
    pub failovers_total: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
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

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct UpstreamPeerStatsSnapshot {
    pub peer_url: String,
    pub attempts_total: u64,
    pub successes_total: u64,
    pub failures_total: u64,
    pub timeouts_total: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct TlsRuntimeSnapshot {
    pub listeners: Vec<TlsListenerStatusSnapshot>,
    pub certificates: Vec<TlsCertificateStatusSnapshot>,
    pub ocsp: Vec<TlsOcspStatusSnapshot>,
    pub vhost_bindings: Vec<TlsVhostBindingSnapshot>,
    pub sni_bindings: Vec<TlsSniBindingSnapshot>,
    pub sni_conflicts: Vec<TlsSniBindingSnapshot>,
    pub default_certificate_bindings: Vec<TlsDefaultCertificateBindingSnapshot>,
    pub reload_boundary: TlsReloadBoundarySnapshot,
    pub expiring_certificate_count: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct TlsListenerStatusSnapshot {
    pub listener_id: String,
    pub listener_name: String,
    pub listen_addr: String,
    pub tls_enabled: bool,
    pub http3_enabled: bool,
    pub http3_listen_addr: Option<String>,
    pub default_certificate: Option<String>,
    pub versions: Option<Vec<String>>,
    pub alpn_protocols: Vec<String>,
    pub http3_versions: Vec<String>,
    pub http3_alpn_protocols: Vec<String>,
    pub client_auth_mode: Option<String>,
    pub client_auth_verify_depth: Option<u32>,
    pub client_auth_crl_configured: bool,
    pub sni_names: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct TlsCertificateStatusSnapshot {
    pub scope: String,
    pub cert_path: String,
    pub server_names: Vec<String>,
    pub subject: Option<String>,
    pub issuer: Option<String>,
    pub serial_number: Option<String>,
    pub san_dns_names: Vec<String>,
    pub fingerprint_sha256: Option<String>,
    pub not_before_unix_ms: Option<u64>,
    pub expires_in_days: Option<i64>,
    pub not_after_unix_ms: Option<u64>,
    pub chain_length: u32,
    pub chain_subjects: Vec<String>,
    pub chain_diagnostics: Vec<String>,
    pub selected_as_default_for_listeners: Vec<String>,
    pub ocsp_staple_configured: bool,
    pub additional_certificate_count: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct TlsOcspStatusSnapshot {
    pub scope: String,
    pub cert_path: String,
    pub ocsp_staple_path: Option<String>,
    pub responder_urls: Vec<String>,
    pub nonce_mode: String,
    pub responder_policy: String,
    pub auto_refresh_enabled: bool,
    pub cache_loaded: bool,
    pub cache_size_bytes: Option<u64>,
    pub cache_modified_unix_ms: Option<u64>,
    pub refreshes_total: u64,
    pub failures_total: u64,
    pub last_error: Option<String>,
    pub last_refresh_unix_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct TlsVhostBindingSnapshot {
    pub listener_name: String,
    pub vhost_id: String,
    pub server_names: Vec<String>,
    pub certificate_scopes: Vec<String>,
    pub fingerprints: Vec<String>,
    pub default_selected: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct TlsSniBindingSnapshot {
    pub listener_name: String,
    pub server_name: String,
    pub certificate_scopes: Vec<String>,
    pub fingerprints: Vec<String>,
    pub default_selected: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct TlsDefaultCertificateBindingSnapshot {
    pub listener_name: String,
    pub server_name: String,
    pub certificate_scopes: Vec<String>,
    pub fingerprints: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct TlsReloadBoundarySnapshot {
    pub reloadable_fields: Vec<String>,
    pub restart_required_fields: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct MtlsStatusSnapshot {
    pub configured_listeners: u32,
    pub optional_listeners: u32,
    pub required_listeners: u32,
    pub authenticated_connections: u64,
    pub authenticated_requests: u64,
    pub anonymous_requests: u64,
    pub handshake_failures_total: u64,
    pub handshake_failures_verify_depth_exceeded: u64,
}

pub fn parse_runtime(snapshot: Option<&NodeSnapshotDetail>) -> Option<RuntimeStatusSnapshot> {
    parse_snapshot_field(snapshot.and_then(|snapshot| snapshot.status.as_ref()))
}

pub fn parse_counters(snapshot: Option<&NodeSnapshotDetail>) -> Option<HttpCountersSnapshot> {
    parse_snapshot_field(snapshot.and_then(|snapshot| snapshot.counters.as_ref()))
}

pub fn parse_traffic(snapshot: Option<&NodeSnapshotDetail>) -> Option<TrafficStatsSnapshot> {
    parse_snapshot_field(snapshot.and_then(|snapshot| snapshot.traffic.as_ref()))
}

pub fn parse_upstream_health(snapshot: Option<&NodeSnapshotDetail>) -> Vec<UpstreamHealthSnapshot> {
    parse_snapshot_field(snapshot.and_then(|snapshot| snapshot.peer_health.as_ref()))
        .unwrap_or_default()
}

pub fn parse_upstream_stats(snapshot: Option<&NodeSnapshotDetail>) -> Vec<UpstreamStatsSnapshot> {
    parse_snapshot_field(snapshot.and_then(|snapshot| snapshot.upstreams.as_ref()))
        .unwrap_or_default()
}

fn parse_snapshot_field<T>(value: Option<&serde_json::Value>) -> Option<T>
where
    T: for<'de> Deserialize<'de>,
{
    value.and_then(|value| serde_json::from_value(value.clone()).ok())
}
