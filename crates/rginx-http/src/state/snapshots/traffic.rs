use serde::{Deserialize, Serialize};

use super::Http3ListenerRuntimeSnapshot;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http3_runtime: Option<Http3ListenerRuntimeSnapshot>,
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
