use std::path::PathBuf;
use std::time::Duration;

use http::{Method, StatusCode};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct CacheZone {
    pub name: String,
    pub path: PathBuf,
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
    pub key: CacheKeyTemplate,
    pub stale_if_error: Option<Duration>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CacheKeyVariable {
    Scheme,
    Host,
    Uri,
    Method,
}

#[derive(Debug, Clone, Copy)]
pub struct CacheKeyRenderContext<'a> {
    pub scheme: &'a str,
    pub host: &'a str,
    pub uri: &'a str,
    pub method: &'a str,
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
}

fn parse_cache_key_variable(name: &str) -> Result<CacheKeyVariable, CacheKeyTemplateError> {
    match name {
        "scheme" => Ok(CacheKeyVariable::Scheme),
        "host" => Ok(CacheKeyVariable::Host),
        "uri" => Ok(CacheKeyVariable::Uri),
        "method" => Ok(CacheKeyVariable::Method),
        _ => Err(CacheKeyTemplateError::UnknownVariable { name: name.to_string() }),
    }
}
