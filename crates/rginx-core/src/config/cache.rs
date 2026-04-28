use std::time::Duration;

use http::header::HeaderName;
use http::{HeaderMap, Method, StatusCode};

mod key_template;
mod predicate;

pub use key_template::CacheKeyTemplateError;

#[derive(Debug, Clone)]
pub struct CacheZone {
    pub name: String,
    pub path: std::path::PathBuf,
    pub max_size_bytes: Option<usize>,
    pub inactive: Duration,
    pub default_ttl: Duration,
    pub max_entry_bytes: usize,
    pub path_levels: Vec<usize>,
    pub loader_batch_entries: usize,
    pub loader_sleep: Duration,
    pub manager_batch_entries: usize,
    pub manager_sleep: Duration,
    pub inactive_cleanup_interval: Duration,
}

#[derive(Debug, Clone)]
pub struct RouteCachePolicy {
    pub zone: String,
    pub methods: Vec<Method>,
    pub statuses: Vec<StatusCode>,
    pub ttl_by_status: Vec<CacheStatusTtlRule>,
    pub key: CacheKeyTemplate,
    pub cache_bypass: Option<CachePredicate>,
    pub no_cache: Option<CachePredicate>,
    pub stale_if_error: Option<Duration>,
    pub use_stale: Vec<CacheUseStaleCondition>,
    pub background_update: bool,
    pub lock_timeout: Duration,
    pub lock_age: Duration,
    pub min_uses: u64,
    pub ignore_headers: Vec<CacheIgnoreHeader>,
    pub range_requests: CacheRangeRequestPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheStatusTtlRule {
    pub statuses: Vec<StatusCode>,
    pub ttl: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CachePredicate {
    Any(Vec<CachePredicate>),
    All(Vec<CachePredicate>),
    Not(Box<CachePredicate>),
    Method(Method),
    HeaderExists(HeaderName),
    HeaderEquals { name: HeaderName, value: String },
    QueryExists(String),
    QueryEquals { name: String, value: String },
    CookieExists(String),
    CookieEquals { name: String, value: String },
    Status(Vec<StatusCode>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheUseStaleCondition {
    Error,
    Timeout,
    Updating,
    Http500,
    Http502,
    Http503,
    Http504,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheIgnoreHeader {
    XAccelExpires,
    Expires,
    CacheControl,
    SetCookie,
    Vary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheRangeRequestPolicy {
    Bypass,
    Cache,
}

#[derive(Debug, Clone, Copy)]
pub struct CachePredicateRequestContext<'a> {
    pub method: &'a Method,
    pub uri: &'a str,
    pub headers: &'a HeaderMap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheKeyTemplate {
    raw: String,
    parts: Vec<CacheKeyTemplatePart>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CacheKeyTemplatePart {
    Literal(String),
    Variable(CacheKeyVariable),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CacheKeyVariable {
    Scheme,
    Host,
    Uri,
    Method,
    Header(HeaderName),
    Query(String),
    Cookie(String),
}

#[derive(Debug, Clone, Copy)]
pub struct CacheKeyRenderContext<'a> {
    pub scheme: &'a str,
    pub host: &'a str,
    pub uri: &'a str,
    pub method: &'a str,
    pub headers: &'a HeaderMap,
}
