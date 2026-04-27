use std::collections::HashSet;

use rginx_core::{Error, ProxyHeaderTemplate, Result};

use crate::model::{HandlerConfig, ProxyHeaderDynamicValueConfig, ProxyHeaderValueConfig};

pub(super) fn validate_handler(
    scope_label: Option<&str>,
    route_scope: &str,
    handler: &HandlerConfig,
    upstream_names: &HashSet<String>,
) -> Result<()> {
    if let HandlerConfig::Proxy { upstream, strip_prefix, proxy_set_headers, .. } = handler {
        if upstream.trim().is_empty() {
            return Err(Error::Config("proxy upstream name must not be empty".to_string()));
        }

        if !upstream_names.contains(upstream) {
            return Err(Error::Config(match scope_label {
                Some(scope_label) => {
                    format!("{scope_label} proxy upstream `{upstream}` is not defined")
                }
                None => format!("proxy upstream `{upstream}` is not defined"),
            }));
        }

        if let Some(prefix) = strip_prefix
            && !prefix.starts_with('/')
        {
            return Err(Error::Config(format!("{route_scope} strip_prefix must start with `/`")));
        }

        let mut proxy_headers = proxy_set_headers.iter().collect::<Vec<_>>();
        proxy_headers.sort_by(|(left, _), (right, _)| left.cmp(right));
        for (name, value) in proxy_headers {
            if name.trim().is_empty() {
                return Err(Error::Config(format!(
                    "{route_scope} proxy_set_headers name must not be empty"
                )));
            }
            if name.parse::<http::header::HeaderName>().is_err() {
                return Err(Error::Config(format!(
                    "{route_scope} proxy_set_headers name `{name}` is invalid"
                )));
            }
            validate_proxy_header_value(route_scope, name, value)?;
        }
    }

    if let HandlerConfig::Return { status, location, .. } = handler {
        if *status < 100 || *status > 599 {
            return Err(Error::Config(format!(
                "{route_scope} return status must be between 100 and 599"
            )));
        }

        if (300..=399).contains(status) && location.trim().is_empty() {
            return Err(Error::Config(format!(
                "{route_scope} return location must not be empty for redirect status {status}"
            )));
        }
    }
    Ok(())
}

fn validate_proxy_header_value(
    route_scope: &str,
    name: &str,
    value: &ProxyHeaderValueConfig,
) -> Result<()> {
    match value {
        ProxyHeaderValueConfig::Static(value) => {
            value.parse::<http::header::HeaderValue>().map_err(|error| {
                Error::Config(format!(
                    "{route_scope} proxy_set_headers value for `{name}` is invalid: {error}"
                ))
            })?;
        }
        ProxyHeaderValueConfig::Dynamic(dynamic) => match dynamic {
            ProxyHeaderDynamicValueConfig::Host
            | ProxyHeaderDynamicValueConfig::Scheme
            | ProxyHeaderDynamicValueConfig::ClientIp
            | ProxyHeaderDynamicValueConfig::RemoteAddr
            | ProxyHeaderDynamicValueConfig::PeerAddr
            | ProxyHeaderDynamicValueConfig::ForwardedFor
            | ProxyHeaderDynamicValueConfig::Remove => {}
            ProxyHeaderDynamicValueConfig::RequestHeader(header_name) => {
                if header_name.trim().is_empty() {
                    return Err(Error::Config(format!(
                        "{route_scope} proxy_set_headers RequestHeader source for `{name}` must not be empty"
                    )));
                }
                header_name.parse::<http::header::HeaderName>().map_err(|error| {
                    Error::Config(format!(
                        "{route_scope} proxy_set_headers RequestHeader source `{header_name}` for `{name}` is invalid: {error}"
                    ))
                })?;
            }
            ProxyHeaderDynamicValueConfig::Template(template) => {
                ProxyHeaderTemplate::parse(template.clone()).map_err(|error| {
                    Error::Config(format!(
                        "{route_scope} proxy_set_headers Template for `{name}` is invalid: {error}"
                    ))
                })?;
            }
        },
    }

    Ok(())
}
