use std::collections::BTreeMap;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{AuditLogSummary, ConfigDiffResponse, NodeLifecycleState, NodeSummary};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DnsDraftValidationState {
    Pending,
    Valid,
    Invalid,
    Published,
}

impl DnsDraftValidationState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Valid => "valid",
            Self::Invalid => "invalid",
            Self::Published => "published",
        }
    }
}

impl FromStr for DnsDraftValidationState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(Self::Pending),
            "valid" => Ok(Self::Valid),
            "invalid" => Ok(Self::Invalid),
            "published" => Ok(Self::Published),
            _ => Err(format!("unknown dns draft validation state `{value}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DnsRecordType {
    #[default]
    A,
    Aaaa,
    Cname,
    Txt,
}

impl DnsRecordType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::Aaaa => "AAAA",
            Self::Cname => "CNAME",
            Self::Txt => "TXT",
        }
    }
}

impl FromStr for DnsRecordType {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_uppercase().as_str() {
            "A" => Ok(Self::A),
            "AAAA" => Ok(Self::Aaaa),
            "CNAME" => Ok(Self::Cname),
            "TXT" => Ok(Self::Txt),
            _ => Err(format!("unknown dns record type `{value}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DnsTargetKind {
    #[default]
    StaticIp,
    Cluster,
    Node,
    Upstream,
}

impl DnsTargetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StaticIp => "static_ip",
            Self::Cluster => "cluster",
            Self::Node => "node",
            Self::Upstream => "upstream",
        }
    }
}

impl FromStr for DnsTargetKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "static_ip" => Ok(Self::StaticIp),
            "cluster" => Ok(Self::Cluster),
            "node" => Ok(Self::Node),
            "upstream" => Ok(Self::Upstream),
            _ => Err(format!("unknown dns target kind `{value}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DnsAnswerTarget {
    pub target_id: String,
    pub kind: DnsTargetKind,
    pub value: String,
    pub weight: u32,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub source_cidrs: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DnsRecordSet {
    pub record_id: String,
    pub name: String,
    pub record_type: DnsRecordType,
    pub ttl_secs: u32,
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default)]
    pub targets: Vec<DnsAnswerTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DnsZoneSpec {
    pub zone_id: String,
    pub zone_name: String,
    #[serde(default)]
    pub records: Vec<DnsRecordSet>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DnsPlan {
    pub cluster_id: String,
    #[serde(default)]
    pub zones: Vec<DnsZoneSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsValidationReport {
    pub valid: bool,
    pub validated_at_unix_ms: u64,
    pub issues: Vec<String>,
    pub zone_count: u32,
    pub record_count: u32,
    pub target_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsDraftSummary {
    pub draft_id: String,
    pub cluster_id: String,
    pub title: String,
    pub summary: String,
    pub base_revision_id: Option<String>,
    pub validation_state: DnsDraftValidationState,
    pub published_revision_id: Option<String>,
    pub created_by: String,
    pub updated_by: String,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsDraftDetail {
    pub draft_id: String,
    pub cluster_id: String,
    pub title: String,
    pub summary: String,
    pub plan: DnsPlan,
    pub base_revision_id: Option<String>,
    pub validation_state: DnsDraftValidationState,
    pub published_revision_id: Option<String>,
    pub created_by: String,
    pub updated_by: String,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub last_validation: Option<DnsValidationReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsRevisionListItem {
    pub revision_id: String,
    pub cluster_id: String,
    pub version_label: String,
    pub summary: String,
    pub created_by: String,
    pub created_at_unix_ms: u64,
    pub published_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsRevisionDetail {
    pub revision_id: String,
    pub cluster_id: String,
    pub version_label: String,
    pub summary: String,
    pub plan: DnsPlan,
    pub validation: DnsValidationReport,
    pub created_by: String,
    pub created_at_unix_ms: u64,
    pub published_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DnsDeploymentStatus {
    Running,
    Paused,
    Succeeded,
    Failed,
    RolledBack,
}

impl DnsDeploymentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::RolledBack => "rolled_back",
        }
    }
}

impl FromStr for DnsDeploymentStatus {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "running" => Ok(Self::Running),
            "paused" => Ok(Self::Paused),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "rolled_back" => Ok(Self::RolledBack),
            _ => Err(format!("unknown dns deployment status `{value}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DnsDeploymentTargetState {
    Pending,
    Active,
    Succeeded,
    Failed,
    Cancelled,
}

impl DnsDeploymentTargetState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl FromStr for DnsDeploymentTargetState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(Self::Pending),
            "active" => Ok(Self::Active),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(format!("unknown dns deployment target state `{value}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsDeploymentSummary {
    pub deployment_id: String,
    pub cluster_id: String,
    pub revision_id: String,
    pub revision_version_label: String,
    pub status: DnsDeploymentStatus,
    pub target_nodes: u32,
    pub healthy_nodes: u32,
    pub failed_nodes: u32,
    pub active_nodes: u32,
    pub pending_nodes: u32,
    pub parallelism: u32,
    pub failure_threshold: u32,
    pub auto_rollback: bool,
    pub promotes_cluster_runtime: bool,
    pub created_by: String,
    pub rollback_of_deployment_id: Option<String>,
    pub rollback_revision_id: Option<String>,
    pub rolled_back_by_deployment_id: Option<String>,
    pub status_reason: Option<String>,
    pub created_at_unix_ms: u64,
    pub started_at_unix_ms: Option<u64>,
    pub finished_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsDeploymentTargetSummary {
    pub target_id: String,
    pub deployment_id: String,
    pub node_id: String,
    pub advertise_addr: String,
    pub node_state: NodeLifecycleState,
    pub desired_revision_id: String,
    pub state: DnsDeploymentTargetState,
    pub batch_index: u32,
    pub last_error: Option<String>,
    pub assigned_at_unix_ms: Option<u64>,
    pub confirmed_at_unix_ms: Option<u64>,
    pub failed_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsDeploymentDetail {
    pub deployment: DnsDeploymentSummary,
    pub revision: DnsRevisionListItem,
    pub rollback_revision: Option<DnsRevisionListItem>,
    pub targets: Vec<DnsDeploymentTargetSummary>,
    pub recent_events: Vec<AuditLogSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateDnsDeploymentRequest {
    pub cluster_id: String,
    pub revision_id: String,
    pub target_node_ids: Option<Vec<String>>,
    pub parallelism: Option<u32>,
    pub failure_threshold: Option<u32>,
    pub auto_rollback: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateDnsDeploymentResponse {
    pub deployment: DnsDeploymentDetail,
    pub reused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateDnsDraftRequest {
    pub cluster_id: String,
    pub title: String,
    pub summary: String,
    pub plan: DnsPlan,
    pub base_revision_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateDnsDraftRequest {
    pub title: String,
    pub summary: String,
    pub plan: DnsPlan,
    pub base_revision_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishDnsDraftRequest {
    pub version_label: String,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishDnsDraftResponse {
    pub draft: DnsDraftDetail,
    pub revision: DnsRevisionDetail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsSimulationRequest {
    pub cluster_id: String,
    pub qname: String,
    pub record_type: DnsRecordType,
    pub source_ip: String,
    pub revision_id: Option<String>,
    pub draft_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsResolvedValue {
    pub value: String,
    pub target_kind: Option<DnsTargetKind>,
    pub target_id: Option<String>,
    pub target_value: Option<String>,
    pub weight: Option<u32>,
    pub source_cidrs: Vec<String>,
    pub node_id: Option<String>,
    pub cluster_id: Option<String>,
    pub healthy: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsSimulationResponse {
    pub cluster_id: String,
    pub qname: String,
    pub record_type: DnsRecordType,
    pub matched_zone: Option<String>,
    pub matched_record_id: Option<String>,
    pub ttl_secs: Option<u32>,
    pub answers: Vec<DnsResolvedValue>,
    pub discarded: Vec<DnsResolvedValue>,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsPublishedSnapshot {
    pub cluster_id: String,
    pub revision_id: String,
    pub version_label: String,
    pub plan: DnsPlan,
    pub nodes: Vec<NodeSummary>,
    pub resolved_upstreams: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeAgentDnsSnapshotResponse {
    pub snapshot: Option<DnsPublishedSnapshot>,
    pub fetched_at_unix_ms: u64,
    pub agent_token: Option<String>,
    pub agent_token_expires_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsRuntimeStatus {
    pub enabled: bool,
    pub cluster_id: String,
    pub udp_bind_addr: Option<String>,
    pub tcp_bind_addr: Option<String>,
    pub published_revision_id: Option<String>,
    pub published_revision_version: Option<String>,
    pub zone_count: u32,
    pub record_count: u32,
    pub query_total: u64,
    pub response_noerror_total: u64,
    pub response_nxdomain_total: u64,
    pub response_servfail_total: u64,
    #[serde(default)]
    pub hot_queries: Vec<DnsRuntimeQueryStat>,
    #[serde(default)]
    pub error_queries: Vec<DnsRuntimeQueryStat>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsRuntimeQueryStat {
    pub zone_name: Option<String>,
    pub qname: String,
    pub record_type: DnsRecordType,
    pub query_total: u64,
    pub answer_total: u64,
    pub response_noerror_total: u64,
    pub response_nxdomain_total: u64,
    pub response_servfail_total: u64,
    pub last_query_at_unix_ms: u64,
}

pub type DnsDiffResponse = ConfigDiffResponse;
