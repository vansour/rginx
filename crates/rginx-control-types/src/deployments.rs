use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{AuditLogSummary, NodeLifecycleState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentStatus {
    Draft,
    Running,
    Paused,
    Succeeded,
    Failed,
    RolledBack,
}

impl DeploymentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::RolledBack => "rolled_back",
        }
    }
}

impl FromStr for DeploymentStatus {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "draft" => Ok(Self::Draft),
            "running" => Ok(Self::Running),
            "paused" => Ok(Self::Paused),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "rolled_back" => Ok(Self::RolledBack),
            _ => Err(format!("unknown deployment status `{value}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentTargetState {
    Pending,
    Dispatched,
    Acknowledged,
    Succeeded,
    Failed,
    Cancelled,
}

impl DeploymentTargetState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Dispatched => "dispatched",
            Self::Acknowledged => "acknowledged",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl FromStr for DeploymentTargetState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(Self::Pending),
            "dispatched" => Ok(Self::Dispatched),
            "acknowledged" => Ok(Self::Acknowledged),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(format!("unknown deployment target state `{value}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentTaskState {
    Pending,
    Acknowledged,
    Succeeded,
    Failed,
    Cancelled,
}

impl DeploymentTaskState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Acknowledged => "acknowledged",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl FromStr for DeploymentTaskState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(Self::Pending),
            "acknowledged" => Ok(Self::Acknowledged),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(format!("unknown deployment task state `{value}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentTaskKind {
    ApplyRevision,
    RollbackRevision,
}

impl DeploymentTaskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApplyRevision => "apply_revision",
            Self::RollbackRevision => "rollback_revision",
        }
    }
}

impl FromStr for DeploymentTaskKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "apply_revision" => Ok(Self::ApplyRevision),
            "rollback_revision" => Ok(Self::RollbackRevision),
            _ => Err(format!("unknown deployment task kind `{value}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigRevisionSummary {
    pub revision_id: String,
    pub cluster_id: String,
    pub version_label: String,
    pub created_at_unix_ms: u64,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentSummary {
    pub deployment_id: String,
    pub cluster_id: String,
    pub revision_id: String,
    pub revision_version_label: String,
    pub status: DeploymentStatus,
    pub target_nodes: u32,
    pub healthy_nodes: u32,
    pub failed_nodes: u32,
    pub in_flight_nodes: u32,
    pub parallelism: u32,
    pub failure_threshold: u32,
    pub auto_rollback: bool,
    pub created_by: String,
    pub rollback_of_deployment_id: Option<String>,
    pub rollback_revision_id: Option<String>,
    pub status_reason: Option<String>,
    pub created_at_unix_ms: u64,
    pub started_at_unix_ms: Option<u64>,
    pub finished_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentTargetSummary {
    pub target_id: String,
    pub deployment_id: String,
    pub node_id: String,
    pub advertise_addr: String,
    pub node_state: NodeLifecycleState,
    pub desired_revision_id: String,
    pub state: DeploymentTargetState,
    pub task_id: Option<String>,
    pub task_kind: Option<DeploymentTaskKind>,
    pub task_state: Option<DeploymentTaskState>,
    pub attempt: u32,
    pub batch_index: u32,
    pub last_error: Option<String>,
    pub dispatched_at_unix_ms: Option<u64>,
    pub acked_at_unix_ms: Option<u64>,
    pub completed_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentDetail {
    pub deployment: DeploymentSummary,
    pub revision: ConfigRevisionSummary,
    pub rollback_revision: Option<ConfigRevisionSummary>,
    pub targets: Vec<DeploymentTargetSummary>,
    pub recent_events: Vec<AuditLogSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateDeploymentRequest {
    pub cluster_id: String,
    pub revision_id: String,
    pub target_node_ids: Option<Vec<String>>,
    pub parallelism: Option<u32>,
    pub failure_threshold: Option<u32>,
    pub auto_rollback: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateDeploymentResponse {
    pub deployment: DeploymentDetail,
    pub reused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeAgentTask {
    pub task_id: String,
    pub deployment_id: String,
    pub target_id: String,
    pub cluster_id: String,
    pub node_id: String,
    pub kind: DeploymentTaskKind,
    pub state: DeploymentTaskState,
    pub revision_id: String,
    pub revision_version_label: String,
    pub source_path: String,
    pub config_text: String,
    pub attempt: u32,
    pub created_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeAgentTaskPollRequest {
    pub node_id: String,
    pub cluster_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeAgentTaskPollResponse {
    pub task: Option<NodeAgentTask>,
    pub polled_at_unix_ms: u64,
    pub agent_token: Option<String>,
    pub agent_token_expires_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeAgentTaskAckRequest {
    pub node_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeAgentTaskAckResponse {
    pub task_id: String,
    pub deployment_id: String,
    pub state: DeploymentTaskState,
    pub acknowledged_at_unix_ms: u64,
    pub agent_token: Option<String>,
    pub agent_token_expires_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeAgentTaskCompleteRequest {
    pub node_id: String,
    pub succeeded: bool,
    pub message: Option<String>,
    pub runtime_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeAgentTaskCompleteResponse {
    pub task_id: String,
    pub deployment_id: String,
    pub state: DeploymentTaskState,
    pub completed_at_unix_ms: u64,
    pub agent_token: Option<String>,
    pub agent_token_expires_at_unix_ms: Option<u64>,
}
