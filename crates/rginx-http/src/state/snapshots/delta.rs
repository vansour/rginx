use serde::{Deserialize, Serialize};

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
