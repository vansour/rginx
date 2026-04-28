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
    pub ttl_secs_by_status: Option<Vec<CacheStatusTtlConfig>>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub cache_bypass: Option<CachePredicateConfig>,
    #[serde(default)]
    pub no_cache: Option<CachePredicateConfig>,
    #[serde(default)]
    pub stale_if_error_secs: Option<u64>,
    #[serde(default)]
    pub use_stale: Option<Vec<CacheUseStaleConditionConfig>>,
    #[serde(default)]
    pub background_update: Option<bool>,
    #[serde(default)]
    pub lock_timeout_secs: Option<u64>,
    #[serde(default)]
    pub lock_age_secs: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CacheStatusTtlConfig {
    pub statuses: Vec<u16>,
    pub ttl_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub enum CachePredicateConfig {
    Any(Vec<CachePredicateConfig>),
    All(Vec<CachePredicateConfig>),
    Not(Box<CachePredicateConfig>),
    Method(String),
    HeaderExists(String),
    HeaderEquals { name: String, value: String },
    QueryExists(String),
    QueryEquals { name: String, value: String },
    CookieExists(String),
    CookieEquals { name: String, value: String },
    Status(u16),
    Statuses(Vec<u16>),
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub enum CacheUseStaleConditionConfig {
    Error,
    Timeout,
    Updating,
    Http500,
    Http502,
    Http503,
    Http504,
}
