use std::collections::HashSet;

use rginx_core::{Error, Result};

use crate::model::{Config, LocationConfig, RouteBufferingPolicyConfig};

mod route;
mod runtime;
mod server;
mod upstream;
mod vhost;

const DEFAULT_GRPC_HEALTH_CHECK_PATH: &str = "/grpc.health.v1.Health/Check";

pub fn validate(config: &Config) -> Result<()> {
    runtime::validate_runtime(&config.runtime)?;

    if config.locations.is_empty() && config.servers.is_empty() {
        return Err(Error::Config(
            "at least one location or virtual host must be configured".to_string(),
        ));
    }
    server::validate_server(&config.server)?;
    server::validate_listeners(&config.listeners, &config.server, &config.servers)?;
    let upstream_names = upstream::validate_upstreams(&config.upstreams)?;
    route::validate_locations(None, &config.locations, &upstream_names)?;

    let mut all_server_names = HashSet::new();
    server::validate_server_names("server", &config.server.server_names, &mut all_server_names)?;
    vhost::validate_virtual_hosts(
        &config.servers,
        &upstream_names,
        &mut all_server_names,
        config.servers.iter().any(|vhost| !vhost.listen.is_empty()),
    )?;
    validate_request_buffering_limits(config)?;

    Ok(())
}

fn validate_request_buffering_limits(config: &Config) -> Result<()> {
    let any_request_buffering_on = config
        .locations
        .iter()
        .chain(config.servers.iter().flat_map(|server| server.locations.iter()))
        .any(route_uses_forced_request_buffering);

    if !any_request_buffering_on {
        return Ok(());
    }

    if config.listeners.is_empty() {
        if config.server.max_request_body_bytes.is_none() {
            return Err(Error::Config(
                "request_buffering=On requires server.max_request_body_bytes when listeners are generated from server.listen or servers[].listen"
                    .to_string(),
            ));
        }

        return Ok(());
    }

    let missing_limits = config
        .listeners
        .iter()
        .filter(|listener| listener.max_request_body_bytes.is_none())
        .map(|listener| listener.name.trim().to_string())
        .collect::<Vec<_>>();

    if missing_limits.is_empty() {
        Ok(())
    } else {
        Err(Error::Config(format!(
            "request_buffering=On requires max_request_body_bytes on every explicit listener; missing for: {}",
            missing_limits.join(", ")
        )))
    }
}

fn route_uses_forced_request_buffering(location: &LocationConfig) -> bool {
    matches!(location.request_buffering, Some(RouteBufferingPolicyConfig::On))
}

#[cfg(test)]
mod tests;
