use rginx_http::{
    HttpCountersSnapshot, RuntimeStatusSnapshot, SnapshotDeltaSnapshot, SnapshotModule,
    TrafficStatsSnapshot, UpstreamHealthSnapshot, UpstreamStatsSnapshot,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminRequest {
    GetSnapshot { include: Option<Vec<SnapshotModule>>, window_secs: Option<u64> },
    GetSnapshotVersion,
    GetDelta { since_version: u64, include: Option<Vec<SnapshotModule>>, window_secs: Option<u64> },
    WaitForSnapshotChange { since_version: u64, timeout_ms: Option<u64> },
    GetStatus,
    GetCounters,
    GetTrafficStats { window_secs: Option<u64> },
    GetPeerHealth,
    GetUpstreamStats { window_secs: Option<u64> },
    GetRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionSnapshot {
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotVersionSnapshot {
    pub snapshot_version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminSnapshot {
    pub schema_version: u32,
    pub snapshot_version: u64,
    pub captured_at_unix_ms: u64,
    pub pid: u32,
    pub binary_version: String,
    pub included_modules: Vec<SnapshotModule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<RuntimeStatusSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counters: Option<HttpCountersSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traffic: Option<TrafficStatsSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_health: Option<Vec<UpstreamHealthSnapshot>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstreams: Option<Vec<UpstreamStatsSnapshot>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
#[allow(clippy::large_enum_variant)]
pub enum AdminResponse {
    Snapshot(AdminSnapshot),
    SnapshotVersion(SnapshotVersionSnapshot),
    Delta(SnapshotDeltaSnapshot),
    Status(RuntimeStatusSnapshot),
    Counters(HttpCountersSnapshot),
    TrafficStats(TrafficStatsSnapshot),
    PeerHealth(Vec<UpstreamHealthSnapshot>),
    UpstreamStats(Vec<UpstreamStatsSnapshot>),
    Revision(RevisionSnapshot),
    Error { message: String },
}
