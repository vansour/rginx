use std::time::Duration;

use http::header::HeaderName;
use http::{HeaderMap, Method, StatusCode};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct CacheZone {
    pub name: String,
    pub path: std::path::PathBuf,
    pub max_size_bytes: Option<usize>,
    pub inactive: Duration,
    pub default_ttl: Duration,
    pub max_entry_bytes: usize,
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

impl CachePredicate {
    pub fn matches_request(&self, request: &CachePredicateRequestContext<'_>) -> bool {
        match self {
            Self::Any(conditions) => {
                conditions.iter().any(|condition| condition.matches_request(request))
            }
            Self::All(conditions) => {
                conditions.iter().all(|condition| condition.matches_request(request))
            }
            Self::Not(condition) => !condition.matches_request(request),
            Self::Method(method) => method == request.method,
            Self::HeaderExists(name) => request.headers.contains_key(name),
            Self::HeaderEquals { name, value } => request
                .headers
                .get_all(name)
                .iter()
                .filter_map(|header| header.to_str().ok())
                .any(|header| header == value),
            Self::QueryExists(name) => query_pairs(request.uri).any(|(key, _)| key == name),
            Self::QueryEquals { name, value } => {
                query_pairs(request.uri).any(|(key, candidate)| key == name && candidate == value)
            }
            Self::CookieExists(name) => cookie_pairs(request.headers).any(|(key, _)| key == name),
            Self::CookieEquals { name, value } => cookie_pairs(request.headers)
                .any(|(key, candidate)| key == name && candidate == value),
            Self::Status(_) => false,
        }
    }

    pub fn matches_response(
        &self,
        request: &CachePredicateRequestContext<'_>,
        status: StatusCode,
    ) -> bool {
        match self {
            Self::Any(conditions) => {
                conditions.iter().any(|condition| condition.matches_response(request, status))
            }
            Self::All(conditions) => {
                conditions.iter().all(|condition| condition.matches_response(request, status))
            }
            Self::Not(condition) => !condition.matches_response(request, status),
            Self::Status(statuses) => statuses.contains(&status),
            _ => self.matches_request(request),
        }
    }
}

impl CacheKeyTemplate {
    pub fn parse(raw: impl Into<String>) -> Result<Self, CacheKeyTemplateError> {
        let raw = raw.into();
        let mut parts = Vec::new();
        let mut literal = String::new();
        let mut index = 0usize;

        while index < raw.len() {
            let remainder = &raw[index..];
            if remainder.starts_with("{{") {
                literal.push('{');
                index += 2;
                continue;
            }
            if remainder.starts_with("}}") {
                literal.push('}');
                index += 2;
                continue;
            }

            let ch = remainder.chars().next().expect("index is inside raw string");
            match ch {
                '{' => {
                    if !literal.is_empty() {
                        parts.push(CacheKeyTemplatePart::Literal(std::mem::take(&mut literal)));
                    }

                    let after_start = &raw[index + 1..];
                    let Some(end) = after_start.find('}') else {
                        return Err(CacheKeyTemplateError::UnclosedVariable {
                            template: raw.clone(),
                        });
                    };
                    let variable = after_start[..end].trim();
                    if variable.is_empty() {
                        return Err(CacheKeyTemplateError::EmptyVariable { template: raw.clone() });
                    }
                    parts.push(CacheKeyTemplatePart::Variable(parse_cache_key_variable(variable)?));
                    index += end + 2;
                }
                '}' => {
                    return Err(CacheKeyTemplateError::UnescapedClose { template: raw.clone() });
                }
                _ => {
                    literal.push(ch);
                    index += ch.len_utf8();
                }
            }
        }

        if !literal.is_empty() {
            parts.push(CacheKeyTemplatePart::Literal(literal));
        }

        Ok(Self { raw, parts })
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    pub fn render(&self, context: &CacheKeyRenderContext<'_>) -> String {
        let mut rendered = String::with_capacity(self.raw.len() + context.uri.len());
        for part in &self.parts {
            match part {
                CacheKeyTemplatePart::Literal(value) => rendered.push_str(value),
                CacheKeyTemplatePart::Variable(variable) => match variable {
                    CacheKeyVariable::Scheme => rendered.push_str(context.scheme),
                    CacheKeyVariable::Host => rendered.push_str(context.host),
                    CacheKeyVariable::Uri => rendered.push_str(context.uri),
                    CacheKeyVariable::Method => rendered.push_str(context.method),
                    CacheKeyVariable::Header(name) => {
                        append_joined_header_values(&mut rendered, context.headers, name);
                    }
                    CacheKeyVariable::Query(name) => {
                        if let Some(value) = query_pairs(context.uri)
                            .find_map(|(key, value)| (key == name).then_some(value))
                        {
                            rendered.push_str(value);
                        }
                    }
                    CacheKeyVariable::Cookie(name) => {
                        if let Some(value) = cookie_pairs(context.headers)
                            .find_map(|(key, value)| (key == name).then_some(value))
                        {
                            rendered.push_str(value);
                        }
                    }
                },
            }
        }
        rendered
    }
}

#[derive(Debug, Error)]
pub enum CacheKeyTemplateError {
    #[error("cache key template `{template}` has an unclosed variable")]
    UnclosedVariable { template: String },
    #[error("cache key template `{template}` has an unescaped closing brace")]
    UnescapedClose { template: String },
    #[error("cache key template `{template}` has an empty variable")]
    EmptyVariable { template: String },
    #[error("cache key template variable `{name}` is not supported")]
    UnknownVariable { name: String },
    #[error("cache key template request header `{name}` is invalid: {source}")]
    InvalidRequestHeader {
        name: String,
        #[source]
        source: http::header::InvalidHeaderName,
    },
    #[error("cache key template variable `{name}` requires a non-empty value")]
    EmptyVariableArgument { name: String },
}

fn parse_cache_key_variable(name: &str) -> Result<CacheKeyVariable, CacheKeyTemplateError> {
    match name {
        "scheme" => Ok(CacheKeyVariable::Scheme),
        "host" => Ok(CacheKeyVariable::Host),
        "uri" => Ok(CacheKeyVariable::Uri),
        "method" => Ok(CacheKeyVariable::Method),
        _ => {
            if let Some(header_name) = name.strip_prefix("header:") {
                let header_name = header_name.trim();
                if header_name.is_empty() {
                    return Err(CacheKeyTemplateError::EmptyVariableArgument {
                        name: name.to_string(),
                    });
                }
                let header_name = header_name.parse::<HeaderName>().map_err(|source| {
                    CacheKeyTemplateError::InvalidRequestHeader {
                        name: header_name.to_string(),
                        source,
                    }
                })?;
                return Ok(CacheKeyVariable::Header(header_name));
            }
            if let Some(query_name) = name.strip_prefix("query:") {
                let query_name = query_name.trim();
                if query_name.is_empty() {
                    return Err(CacheKeyTemplateError::EmptyVariableArgument {
                        name: name.to_string(),
                    });
                }
                return Ok(CacheKeyVariable::Query(query_name.to_string()));
            }
            if let Some(cookie_name) = name.strip_prefix("cookie:") {
                let cookie_name = cookie_name.trim();
                if cookie_name.is_empty() {
                    return Err(CacheKeyTemplateError::EmptyVariableArgument {
                        name: name.to_string(),
                    });
                }
                return Ok(CacheKeyVariable::Cookie(cookie_name.to_string()));
            }
            Err(CacheKeyTemplateError::UnknownVariable { name: name.to_string() })
        }
    }
}

fn append_joined_header_values(rendered: &mut String, headers: &HeaderMap, name: &HeaderName) {
    let mut values =
        headers.get_all(name).iter().filter_map(|header| header.to_str().ok()).peekable();
    while let Some(value) = values.next() {
        rendered.push_str(value);
        if values.peek().is_some() {
            rendered.push(',');
        }
    }
}

fn query_pairs(uri: &str) -> impl Iterator<Item = (&str, &str)> {
    uri.split_once('?')
        .map(|(_, query)| query.split('&'))
        .into_iter()
        .flatten()
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (name, value)
        })
}

fn cookie_pairs(headers: &HeaderMap) -> impl Iterator<Item = (&str, &str)> {
    headers
        .get_all(http::header::COOKIE)
        .iter()
        .filter_map(|header| header.to_str().ok())
        .flat_map(|header| header.split(';'))
        .map(str::trim)
        .filter(|cookie| !cookie.is_empty())
        .map(|cookie| {
            let (name, value) = cookie.split_once('=').unwrap_or((cookie, ""));
            (name.trim(), value.trim())
        })
}
