use serde_json::Value;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditLogSummary {
    pub audit_id: String,
    pub request_id: String,
    pub cluster_id: Option<String>,
    pub actor_id: String,
    pub action: String,
    pub resource_type: String,
    pub resource_id: String,
    pub result: String,
    pub created_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub audit_id: String,
    pub request_id: String,
    pub cluster_id: Option<String>,
    pub actor_id: String,
    pub action: String,
    pub resource_type: String,
    pub resource_id: String,
    pub result: String,
    pub details: Value,
    pub created_at_unix_ms: u64,
}
