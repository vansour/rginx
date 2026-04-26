use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReloadOutcomeSnapshot {
    Success { revision: u64 },
    Failure { error: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReloadResultSnapshot {
    pub finished_at_unix_ms: u64,
    pub outcome: ReloadOutcomeSnapshot,
    pub tls_certificate_changes: Vec<String>,
    pub active_revision: u64,
    pub rollback_preserved_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReloadStatusSnapshot {
    pub attempts_total: u64,
    pub successes_total: u64,
    pub failures_total: u64,
    pub last_result: Option<ReloadResultSnapshot>,
}
