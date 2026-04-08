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
    pub active_connections: usize,
    pub reload: ReloadStatusSnapshot,
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
