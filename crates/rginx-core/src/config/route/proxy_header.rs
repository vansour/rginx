use std::net::{IpAddr, SocketAddr};

use http::{HeaderMap, HeaderName, HeaderValue};
use thiserror::Error;

/// Runtime value used by `proxy_set_headers`.
///
/// Missing dynamic sources such as `RequestHeader("x-name")` are treated as
/// no-ops by the proxy layer. Use `Remove` when a header should be deleted
/// deliberately.
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
    Remove,
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
            Self::Remove => Ok(None),
        }
    }

    pub fn removes_header(&self) -> bool {
        matches!(self, Self::Remove)
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
        let mut literal = String::new();
        let mut index = 0;

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
                        parts.push(ProxyHeaderTemplatePart::Literal(std::mem::take(&mut literal)));
                    }

                    let after_start = &raw[index + 1..];
                    let Some(end) = after_start.find('}') else {
                        return Err(ProxyHeaderTemplateError::UnclosedVariable {
                            template: raw.clone(),
                        });
                    };
                    let variable = after_start[..end].trim();
                    if variable.is_empty() {
                        return Err(ProxyHeaderTemplateError::EmptyVariable {
                            template: raw.clone(),
                        });
                    }
                    parts.push(ProxyHeaderTemplatePart::Variable(parse_proxy_header_variable(
                        variable,
                    )?));
                    index += end + 2;
                }
                '}' => {
                    return Err(ProxyHeaderTemplateError::UnescapedClose { template: raw.clone() });
                }
                _ => {
                    literal.push(ch);
                    index += ch.len_utf8();
                }
            }
        }

        if !literal.is_empty() {
            parts.push(ProxyHeaderTemplatePart::Literal(literal));
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
        let mut rendered = Vec::new();
        for part in &self.parts {
            match part {
                ProxyHeaderTemplatePart::Literal(value) => {
                    rendered.extend_from_slice(value.as_bytes());
                }
                ProxyHeaderTemplatePart::Variable(variable) => {
                    variable.append_to(context, &mut rendered);
                }
            }
        }
        HeaderValue::from_bytes(&rendered)
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
    fn append_to(&self, context: &ProxyHeaderRenderContext<'_>, rendered: &mut Vec<u8>) {
        match self {
            Self::Host => rendered.extend_from_slice(context.host_value().as_bytes()),
            Self::Scheme => rendered.extend_from_slice(context.scheme.as_bytes()),
            Self::ClientIp | Self::RemoteAddr => {
                rendered.extend_from_slice(context.client_ip.to_string().as_bytes());
            }
            Self::PeerAddr => rendered.extend_from_slice(context.peer_addr.to_string().as_bytes()),
            Self::ForwardedFor => rendered.extend_from_slice(context.forwarded_for.as_bytes()),
            Self::RequestHeader(name) => {
                if let Some(value) = context.original_headers.get(name) {
                    rendered.extend_from_slice(value.as_bytes());
                }
            }
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
    #[error("proxy header template `{template}` has an unescaped closing brace")]
    UnescapedClose { template: String },
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
