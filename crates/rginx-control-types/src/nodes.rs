use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AuditLogSummary;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeLifecycleState {
    Provisioning,
    Online,
    Draining,
    Offline,
    Drifted,
}

impl NodeLifecycleState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Provisioning => "provisioning",
            Self::Online => "online",
            Self::Draining => "draining",
            Self::Offline => "offline",
            Self::Drifted => "drifted",
        }
    }
}

impl FromStr for NodeLifecycleState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "provisioning" => Ok(Self::Provisioning),
            "online" => Ok(Self::Online),
            "draining" => Ok(Self::Draining),
            "offline" => Ok(Self::Offline),
            "drifted" => Ok(Self::Drifted),
            _ => Err(format!("unknown node lifecycle state `{value}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NodeRuntimeReport {
    pub snapshot_version: Option<u64>,
    pub revision: Option<u64>,
    pub pid: Option<u32>,
    pub listener_count: Option<u32>,
    pub active_connections: Option<u32>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeSummary {
    pub node_id: String,
    pub cluster_id: String,
    pub advertise_addr: String,
    pub role: String,
    pub state: NodeLifecycleState,
    pub running_version: String,
    pub admin_socket_path: String,
    pub last_seen_unix_ms: u64,
    pub last_snapshot_version: Option<u64>,
    pub runtime_revision: Option<u64>,
    pub runtime_pid: Option<u32>,
    pub listener_count: Option<u32>,
    pub active_connections: Option<u32>,
    pub status_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeAgentReport {
    pub node_id: String,
    pub cluster_id: String,
    pub advertise_addr: String,
    pub role: String,
    pub running_version: String,
    pub admin_socket_path: String,
    pub state: NodeLifecycleState,
    pub observed_at_unix_ms: u64,
    pub runtime: NodeRuntimeReport,
}

pub type NodeAgentRegistrationRequest = NodeAgentReport;
pub type NodeAgentHeartbeatRequest = NodeAgentReport;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeAgentWriteResponse {
    pub node: NodeSummary,
    pub accepted_at_unix_ms: u64,
    pub agent_token: Option<String>,
    pub agent_token_expires_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeSnapshotIngestRequest {
    pub node_id: String,
    pub cluster_id: String,
    pub observed_at_unix_ms: u64,
    pub snapshot_version: u64,
    pub schema_version: u32,
    pub captured_at_unix_ms: u64,
    pub pid: u32,
    pub binary_version: String,
    pub included_modules: Vec<String>,
    pub status: Option<Value>,
    pub counters: Option<Value>,
    pub traffic: Option<Value>,
    pub peer_health: Option<Value>,
    pub upstreams: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeSnapshotMeta {
    pub node_id: String,
    pub snapshot_version: u64,
    pub schema_version: u32,
    pub captured_at_unix_ms: u64,
    pub pid: u32,
    pub binary_version: String,
    pub included_modules: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeSnapshotDetail {
    pub node_id: String,
    pub snapshot_version: u64,
    pub schema_version: u32,
    pub captured_at_unix_ms: u64,
    pub pid: u32,
    pub binary_version: String,
    pub included_modules: Vec<String>,
    pub status: Option<Value>,
    pub counters: Option<Value>,
    pub traffic: Option<Value>,
    pub peer_health: Option<Value>,
    pub upstreams: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeSnapshotIngestResponse {
    pub snapshot: NodeSnapshotMeta,
    pub accepted_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeDetailResponse {
    pub node: NodeSummary,
    pub latest_snapshot: Option<NodeSnapshotDetail>,
    pub recent_snapshots: Vec<NodeSnapshotMeta>,
    pub recent_events: Vec<AuditLogSummary>,
}
