use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use ipnet::IpNet;
use regex::{Regex, RegexBuilder};
use thiserror::Error;

use super::upstream::Upstream;

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
            Self::Regex(regex) => (2, regex.pattern.len()),
            Self::Prefix(path) => (1, path.len()),
        }
    }

    pub fn id_fragment(&self) -> String {
        match self {
            Self::Exact(path) => format!("exact:{path}"),
            Self::Prefix(path) => format!("prefix:{path}"),
            Self::Regex(regex) => {
                if regex.case_insensitive {
                    format!("regex:i:{}", regex.pattern)
                } else {
                    format!("regex:{}", regex.pattern)
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct RouteRegexMatcher {
    pub pattern: String,
    pub case_insensitive: bool,
    regex: Regex,
}

impl RouteRegexMatcher {
    pub fn new(pattern: String, case_insensitive: bool) -> Result<Self, RouteRegexError> {
        let regex =
            RegexBuilder::new(&pattern).case_insensitive(case_insensitive).build().map_err(
                |source| RouteRegexError::InvalidPattern { pattern: pattern.clone(), source },
            )?;

        Ok(Self { pattern, case_insensitive, regex })
    }

    pub fn matches(&self, path: &str) -> bool {
        self.regex.is_match(path)
    }
}

impl PartialEq for RouteRegexMatcher {
    fn eq(&self, other: &Self) -> bool {
        self.pattern == other.pattern && self.case_insensitive == other.case_insensitive
    }
}

impl Eq for RouteRegexMatcher {}

#[derive(Debug, Error)]
pub enum RouteRegexError {
    #[error("route regex pattern `{pattern}` is invalid: {source}")]
    InvalidPattern {
        pattern: String,
        #[source]
        source: regex::Error,
    },
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyHeaderValue {
    Static(HeaderValue),
    Host,
    Scheme,
    ClientIp,
    RemoteAddr,
    PeerAddr,
    ForwardedFor,
    RequestHeader(HeaderName),
    Template(ProxyHeaderTemplate),
}

impl ProxyHeaderValue {
    pub fn render(
        &self,
        context: &ProxyHeaderRenderContext<'_>,
    ) -> Result<Option<HeaderValue>, http::header::InvalidHeaderValue> {
        match self {
            Self::Static(value) => Ok(Some(value.clone())),
            Self::Host => HeaderValue::from_str(context.host_value()).map(Some),
            Self::Scheme => HeaderValue::from_str(context.scheme).map(Some),
            Self::ClientIp | Self::RemoteAddr => {
                HeaderValue::from_str(&context.client_ip.to_string()).map(Some)
            }
            Self::PeerAddr => HeaderValue::from_str(&context.peer_addr.to_string()).map(Some),
            Self::ForwardedFor => HeaderValue::from_str(context.forwarded_for).map(Some),
            Self::RequestHeader(name) => Ok(context.original_headers.get(name).cloned()),
            Self::Template(template) => template.render(context).map(Some),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyHeaderTemplate {
    raw: String,
    parts: Vec<ProxyHeaderTemplatePart>,
}

impl ProxyHeaderTemplate {
    pub fn parse(raw: String) -> Result<Self, ProxyHeaderTemplateError> {
        let mut parts = Vec::new();
        let mut remainder = raw.as_str();

        while let Some(start) = remainder.find('{') {
            if start > 0 {
                parts.push(ProxyHeaderTemplatePart::Literal(remainder[..start].to_string()));
            }
            let after_start = &remainder[start + 1..];
            let Some(end) = after_start.find('}') else {
                return Err(ProxyHeaderTemplateError::UnclosedVariable { template: raw });
            };
            let variable = after_start[..end].trim();
            if variable.is_empty() {
                return Err(ProxyHeaderTemplateError::EmptyVariable { template: raw });
            }
            parts.push(ProxyHeaderTemplatePart::Variable(parse_proxy_header_variable(variable)?));
            remainder = &after_start[end + 1..];
        }

        if !remainder.is_empty() {
            parts.push(ProxyHeaderTemplatePart::Literal(remainder.to_string()));
        }

        Ok(Self { raw, parts })
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    fn render(
        &self,
        context: &ProxyHeaderRenderContext<'_>,
    ) -> Result<HeaderValue, http::header::InvalidHeaderValue> {
        let mut rendered = String::new();
        for part in &self.parts {
            match part {
                ProxyHeaderTemplatePart::Literal(value) => rendered.push_str(value),
                ProxyHeaderTemplatePart::Variable(variable) => {
                    rendered.push_str(&variable.render(context));
                }
            }
        }
        HeaderValue::from_str(&rendered)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProxyHeaderTemplatePart {
    Literal(String),
    Variable(ProxyHeaderVariable),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProxyHeaderVariable {
    Host,
    Scheme,
    ClientIp,
    RemoteAddr,
    PeerAddr,
    ForwardedFor,
    RequestHeader(HeaderName),
}

impl ProxyHeaderVariable {
    fn render(&self, context: &ProxyHeaderRenderContext<'_>) -> String {
        match self {
            Self::Host => context.host_value().to_string(),
            Self::Scheme => context.scheme.to_string(),
            Self::ClientIp | Self::RemoteAddr => context.client_ip.to_string(),
            Self::PeerAddr => context.peer_addr.to_string(),
            Self::ForwardedFor => context.forwarded_for.to_string(),
            Self::RequestHeader(name) => context
                .original_headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProxyHeaderRenderContext<'a> {
    pub original_headers: &'a HeaderMap,
    pub original_host: Option<&'a HeaderValue>,
    pub upstream_authority: &'a str,
    pub client_ip: IpAddr,
    pub peer_addr: SocketAddr,
    pub forwarded_for: &'a str,
    pub scheme: &'a str,
}

impl ProxyHeaderRenderContext<'_> {
    fn host_value(&self) -> &str {
        self.original_host.and_then(|value| value.to_str().ok()).unwrap_or(self.upstream_authority)
    }
}

#[derive(Debug, Error)]
pub enum ProxyHeaderTemplateError {
    #[error("proxy header template `{template}` has an unclosed variable")]
    UnclosedVariable { template: String },
    #[error("proxy header template `{template}` has an empty variable")]
    EmptyVariable { template: String },
    #[error("proxy header template variable `{name}` is not supported")]
    UnknownVariable { name: String },
    #[error("proxy header template request header `{name}` is invalid: {source}")]
    InvalidRequestHeader {
        name: String,
        #[source]
        source: http::header::InvalidHeaderName,
    },
}

fn parse_proxy_header_variable(
    variable: &str,
) -> Result<ProxyHeaderVariable, ProxyHeaderTemplateError> {
    match variable {
        "host" => Ok(ProxyHeaderVariable::Host),
        "scheme" => Ok(ProxyHeaderVariable::Scheme),
        "client_ip" => Ok(ProxyHeaderVariable::ClientIp),
        "remote_addr" => Ok(ProxyHeaderVariable::RemoteAddr),
        "peer_addr" => Ok(ProxyHeaderVariable::PeerAddr),
        "forwarded_for" => Ok(ProxyHeaderVariable::ForwardedFor),
        _ => {
            let Some(header_name) = variable.strip_prefix("header:") else {
                return Err(ProxyHeaderTemplateError::UnknownVariable {
                    name: variable.to_string(),
                });
            };
            let header_name = header_name.parse::<HeaderName>().map_err(|source| {
                ProxyHeaderTemplateError::InvalidRequestHeader {
                    name: header_name.to_string(),
                    source,
                }
            })?;
            Ok(ProxyHeaderVariable::RequestHeader(header_name))
        }
    }
}
