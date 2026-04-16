use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigDraftValidationState {
    Pending,
    Valid,
    Invalid,
    Published,
}

impl ConfigDraftValidationState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Valid => "valid",
            Self::Invalid => "invalid",
            Self::Published => "published",
        }
    }
}

impl FromStr for ConfigDraftValidationState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(Self::Pending),
            "valid" => Ok(Self::Valid),
            "invalid" => Ok(Self::Invalid),
            "published" => Ok(Self::Published),
            _ => Err(format!("unknown config draft validation state `{value}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledListenerBindingSummary {
    pub binding_name: String,
    pub transport: String,
    pub listen_addr: String,
    pub protocols: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledListenerSummary {
    pub listener_id: String,
    pub listener_name: String,
    pub listen_addr: String,
    pub binding_count: u32,
    pub tls_enabled: bool,
    pub http3_enabled: bool,
    pub default_certificate: Option<String>,
    pub bindings: Vec<CompiledListenerBindingSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledTlsSummary {
    pub listener_tls_profiles: u32,
    pub vhost_tls_overrides: u32,
    pub default_certificate_bindings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigCompileSummary {
    pub listener_model: String,
    pub listener_count: u32,
    pub listener_binding_count: u32,
    pub total_vhost_count: u32,
    pub total_route_count: u32,
    pub upstream_count: u32,
    pub worker_threads: Option<u32>,
    pub accept_workers: u32,
    pub tls_enabled: bool,
    pub http3_enabled: bool,
    pub http3_early_data_enabled_listeners: u32,
    pub default_server_names: Vec<String>,
    pub upstream_names: Vec<String>,
    pub listeners: Vec<CompiledListenerSummary>,
    pub tls: CompiledTlsSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigValidationReport {
    pub valid: bool,
    pub validated_at_unix_ms: u64,
    pub normalized_source_path: String,
    pub issues: Vec<String>,
    pub summary: Option<ConfigCompileSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigRevisionListItem {
    pub revision_id: String,
    pub cluster_id: String,
    pub version_label: String,
    pub summary: String,
    pub created_by: String,
    pub created_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigRevisionDetail {
    pub revision_id: String,
    pub cluster_id: String,
    pub version_label: String,
    pub summary: String,
    pub created_by: String,
    pub created_at_unix_ms: u64,
    pub source_path: String,
    pub config_text: String,
    pub compile_summary: Option<ConfigCompileSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigDraftSummary {
    pub draft_id: String,
    pub cluster_id: String,
    pub title: String,
    pub summary: String,
    pub base_revision_id: Option<String>,
    pub validation_state: ConfigDraftValidationState,
    pub published_revision_id: Option<String>,
    pub created_by: String,
    pub updated_by: String,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigDraftDetail {
    pub draft_id: String,
    pub cluster_id: String,
    pub title: String,
    pub summary: String,
    pub source_path: String,
    pub config_text: String,
    pub base_revision_id: Option<String>,
    pub validation_state: ConfigDraftValidationState,
    pub published_revision_id: Option<String>,
    pub created_by: String,
    pub updated_by: String,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub last_validation: Option<ConfigValidationReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateConfigDraftRequest {
    pub cluster_id: String,
    pub title: String,
    pub summary: String,
    pub source_path: String,
    pub config_text: String,
    pub base_revision_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateConfigDraftRequest {
    pub title: String,
    pub summary: String,
    pub source_path: String,
    pub config_text: String,
    pub base_revision_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishConfigDraftRequest {
    pub version_label: String,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishConfigDraftResponse {
    pub draft: ConfigDraftDetail,
    pub revision: ConfigRevisionDetail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigDiffLineKind {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigDiffLine {
    pub kind: ConfigDiffLineKind,
    pub left_line_number: Option<u32>,
    pub right_line_number: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigDiffResponse {
    pub left_label: String,
    pub right_label: String,
    pub changed: bool,
    pub lines: Vec<ConfigDiffLine>,
}
