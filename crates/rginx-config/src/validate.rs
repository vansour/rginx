use std::collections::HashSet;

use rginx_core::{Error, Result};

use crate::model::{Config, HandlerConfig, MatcherConfig, UpstreamTlsConfig};

pub fn validate(config: &Config) -> Result<()> {
    if config.runtime.shutdown_timeout_secs == 0 {
        return Err(Error::Config(
            "runtime.shutdown_timeout_secs must be greater than 0".to_string(),
        ));
    }

    if config.locations.is_empty() {
        return Err(Error::Config("at least one location must be configured".to_string()));
    }

    let mut upstream_names = HashSet::new();
    for upstream in &config.upstreams {
        if upstream.name.trim().is_empty() {
            return Err(Error::Config("upstream name must not be empty".to_string()));
        }

        if !upstream_names.insert(upstream.name.clone()) {
            return Err(Error::Config(format!("duplicate upstream `{}`", upstream.name)));
        }

        if upstream.peers.is_empty() {
            return Err(Error::Config(format!(
                "upstream `{}` must define at least one peer",
                upstream.name
            )));
        }

        for peer in &upstream.peers {
            if peer.url.trim().is_empty() {
                return Err(Error::Config(format!(
                    "upstream `{}` contains an empty peer url",
                    upstream.name
                )));
            }
        }

        if let Some(UpstreamTlsConfig::CustomCa { ca_cert_path }) = &upstream.tls {
            if ca_cert_path.trim().is_empty() {
                return Err(Error::Config(format!(
                    "upstream `{}` custom CA path must not be empty",
                    upstream.name
                )));
            }
        }

        if let Some(server_name_override) = &upstream.server_name_override {
            if server_name_override.trim().is_empty() {
                return Err(Error::Config(format!(
                    "upstream `{}` server_name_override must not be empty",
                    upstream.name
                )));
            }
        }
    }

    let mut exact_routes = HashSet::new();

    for location in &config.locations {
        match &location.matcher {
            MatcherConfig::Exact(path) | MatcherConfig::Prefix(path) => {
                if !path.starts_with('/') {
                    return Err(Error::Config(format!(
                        "route matcher `{path}` must start with `/`"
                    )));
                }
            }
        }

        if let MatcherConfig::Exact(path) = &location.matcher {
            if !exact_routes.insert(path.clone()) {
                return Err(Error::Config(format!("duplicate exact route `{path}`")));
            }
        }

        if let HandlerConfig::Proxy { upstream } = &location.handler {
            if upstream.trim().is_empty() {
                return Err(Error::Config("proxy upstream name must not be empty".to_string()));
            }

            if !upstream_names.contains(upstream) {
                return Err(Error::Config(format!("proxy upstream `{upstream}` is not defined")));
            }
        }
    }

    Ok(())
}
