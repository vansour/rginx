use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AcmeChallengeType {
    Http01,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcmeSettings {
    pub directory_url: String,
    pub contacts: Vec<String>,
    pub state_dir: PathBuf,
    pub renew_before: Duration,
    pub poll_interval: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedCertificateSpec {
    pub scope: String,
    pub domains: Vec<String>,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub challenge: AcmeChallengeType,
}
