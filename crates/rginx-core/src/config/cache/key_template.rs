use http::HeaderMap;
use thiserror::Error;

use super::predicate::{cookie_pairs, query_pairs};
use super::*;

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

    pub fn references_method(&self) -> bool {
        self.parts
            .iter()
            .any(|part| matches!(part, CacheKeyTemplatePart::Variable(CacheKeyVariable::Method)))
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
