use rginx_http::{
    CachePurgeResult, CacheStatsSnapshot, HttpCountersSnapshot, RuntimeStatusSnapshot,
    SnapshotDeltaSnapshot, SnapshotModule, TrafficStatsSnapshot, UpstreamHealthSnapshot,
    UpstreamStatsSnapshot,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminRequest {
    GetSnapshot { include: Option<Vec<SnapshotModule>>, window_secs: Option<u64> },
    GetSnapshotVersion,
    GetDelta { since_version: u64, include: Option<Vec<SnapshotModule>>, window_secs: Option<u64> },
    WaitForSnapshotChange { since_version: u64, timeout_ms: Option<u64> },
    GetStatus,
    GetCacheStats,
    GetCounters,
    GetTrafficStats { window_secs: Option<u64> },
    GetPeerHealth,
    GetUpstreamStats { window_secs: Option<u64> },
    PurgeCacheZone { zone_name: String },
    PurgeCacheKey { zone_name: String, key: String },
    PurgeCachePrefix { zone_name: String, prefix: String },
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheStatsSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
#[allow(clippy::large_enum_variant)]
pub enum AdminResponse {
    Snapshot(AdminSnapshot),
    SnapshotVersion(SnapshotVersionSnapshot),
    Delta(SnapshotDeltaSnapshot),
    Status(RuntimeStatusSnapshot),
    CacheStats(CacheStatsSnapshot),
    Counters(HttpCountersSnapshot),
    TrafficStats(TrafficStatsSnapshot),
    PeerHealth(Vec<UpstreamHealthSnapshot>),
    UpstreamStats(Vec<UpstreamStatsSnapshot>),
    CachePurge(CachePurgeResult),
    Revision(RevisionSnapshot),
    Error { message: String },
}
