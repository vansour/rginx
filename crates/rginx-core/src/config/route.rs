use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use http::{HeaderName, StatusCode};
use ipnet::IpNet;

use super::cache::RouteCachePolicy;
use super::upstream::Upstream;

mod proxy_header;
mod regex_matcher;

pub use proxy_header::{
    ProxyHeaderRenderContext, ProxyHeaderTemplate, ProxyHeaderTemplateError, ProxyHeaderValue,
};
pub use regex_matcher::{RouteRegexError, RouteRegexMatcher};

#[derive(Debug, Clone)]
pub struct Route {
    pub id: String,
    pub matcher: RouteMatcher,
    pub grpc_match: Option<GrpcRouteMatch>,
    pub action: RouteAction,
    pub access_control: RouteAccessControl,
    pub rate_limit: Option<RouteRateLimit>,
    pub allow_early_data: bool,
    pub request_buffering: RouteBufferingPolicy,
    pub response_buffering: RouteBufferingPolicy,
    pub compression: RouteCompressionPolicy,
    pub compression_min_bytes: Option<usize>,
    pub compression_content_types: Vec<String>,
    pub streaming_response_idle_timeout: Option<Duration>,
    pub cache: Option<RouteCachePolicy>,
}

impl Route {
    pub fn priority(&self) -> (u8, usize, u8) {
        let (matcher_rank, matcher_len) = self.matcher.priority();
        let grpc_rank = self.grpc_match.as_ref().map_or(0, |grpc_match| grpc_match.priority());
        (matcher_rank, matcher_len, grpc_rank)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteAccessControl {
    pub allow_cidrs: Vec<IpNet>,
    pub deny_cidrs: Vec<IpNet>,
}

impl RouteAccessControl {
    pub fn new(allow_cidrs: Vec<IpNet>, deny_cidrs: Vec<IpNet>) -> Self {
        Self { allow_cidrs, deny_cidrs }
    }

    pub fn allows(&self, ip: IpAddr) -> bool {
        if self.deny_cidrs.iter().any(|cidr| cidr.contains(&ip)) {
            return false;
        }

        self.allow_cidrs.is_empty() || self.allow_cidrs.iter().any(|cidr| cidr.contains(&ip))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteRateLimit {
    pub requests_per_sec: u32,
    pub burst: u32,
}

impl RouteRateLimit {
    pub fn new(requests_per_sec: u32, burst: u32) -> Self {
        Self { requests_per_sec, burst }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RouteBufferingPolicy {
    #[default]
    Auto,
    On,
    Off,
}

impl RouteBufferingPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RouteCompressionPolicy {
    Off,
    #[default]
    Auto,
    Force,
}

impl RouteCompressionPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::Force => "force",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteMatcher {
    Exact(String),
    Prefix(String),
    Regex(RouteRegexMatcher),
}

impl RouteMatcher {
    pub fn matches(&self, path: &str) -> bool {
        match self {
            Self::Exact(expected) => path == expected,
            Self::Prefix(prefix) if prefix == "/" => true,
            Self::Prefix(prefix) => {
                path == prefix
                    || path.strip_prefix(prefix).is_some_and(|remainder| remainder.starts_with('/'))
            }
            Self::Regex(regex) => regex.matches(path),
        }
    }

    pub fn priority(&self) -> (u8, usize) {
        match self {
            Self::Exact(path) => (3, path.len()),
            // Regex routes keep declaration order among themselves. compile_routes uses a stable
            // sort, so equal regex priorities do not reorder overlapping regex matchers.
            Self::Regex(_) => (2, 0),
            Self::Prefix(path) => (1, path.len()),
        }
    }

    pub fn id_fragment(&self) -> String {
        match self {
            Self::Exact(path) => format!("exact:{path}"),
            Self::Prefix(path) => format!("prefix:{path}"),
            Self::Regex(regex) => {
                if regex.case_insensitive() {
                    format!("regex:i:{}", regex.pattern())
                } else {
                    format!("regex:{}", regex.pattern())
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrpcRouteMatch {
    pub service: Option<String>,
    pub method: Option<String>,
}

impl GrpcRouteMatch {
    pub fn matches(&self, service: &str, method: &str) -> bool {
        self.service.as_deref().is_none_or(|expected| expected == service)
            && self.method.as_deref().is_none_or(|expected| expected == method)
    }

    pub fn priority(&self) -> u8 {
        u8::from(self.service.is_some()) + u8::from(self.method.is_some())
    }

    pub fn id_fragment(&self) -> String {
        let mut fragments = Vec::new();
        if let Some(service) = &self.service {
            fragments.push(format!("service={service}"));
        }
        if let Some(method) = &self.method {
            fragments.push(format!("method={method}"));
        }
        format!("grpc:{}", fragments.join(","))
    }
}

#[derive(Debug, Clone)]
pub enum RouteAction {
    Proxy(ProxyTarget),
    Return(ReturnAction),
}

#[derive(Debug, Clone)]
pub struct ProxyTarget {
    pub upstream_name: String,
    pub upstream: Arc<Upstream>,
    pub preserve_host: bool,
    pub strip_prefix: Option<String>,
    pub proxy_set_headers: Vec<(HeaderName, ProxyHeaderValue)>,
}

#[derive(Debug, Clone)]
pub struct ReturnAction {
    pub status: StatusCode,
    pub location: String,
    pub body: Option<String>,
}
