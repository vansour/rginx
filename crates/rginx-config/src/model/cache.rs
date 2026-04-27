use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct CacheZoneConfig {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub max_size_bytes: Option<u64>,
    #[serde(default)]
    pub inactive_secs: Option<u64>,
    #[serde(default)]
    pub default_ttl_secs: Option<u64>,
    #[serde(default)]
    pub max_entry_bytes: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CacheRouteConfig {
    pub zone: String,
    #[serde(default)]
    pub methods: Option<Vec<String>>,
    #[serde(default)]
    pub statuses: Option<Vec<u16>>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub stale_if_error_secs: Option<u64>,
}
