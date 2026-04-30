use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcmeRuntimeSnapshot {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory_url: Option<String>,
    pub managed_certificates: Vec<AcmeManagedCertificateSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcmeManagedCertificateSnapshot {
    pub scope: String,
    pub domains: Vec<String>,
    pub managed: bool,
    pub challenge_type: String,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_success_unix_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_renewal_unix_ms: Option<u64>,
    pub refreshes_total: u64,
    pub failures_total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_unix_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}
