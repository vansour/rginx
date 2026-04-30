use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct AcmeConfig {
    pub directory_url: String,
    #[serde(default)]
    pub contacts: Vec<String>,
    pub state_dir: String,
    #[serde(default)]
    pub renew_before_days: Option<u64>,
    #[serde(default)]
    pub poll_interval_secs: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VirtualHostAcmeConfig {
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub challenge: Option<AcmeChallengeConfig>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Hash)]
pub enum AcmeChallengeConfig {
    Http01,
}
