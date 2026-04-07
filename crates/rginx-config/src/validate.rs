use std::collections::HashSet;

use rginx_core::{Error, Result};

use crate::model::Config;

mod route;
mod runtime;
mod server;
mod upstream;
mod vhost;

const DEFAULT_GRPC_HEALTH_CHECK_PATH: &str = "/grpc.health.v1.Health/Check";

pub fn validate(config: &Config) -> Result<()> {
    runtime::validate_runtime(&config.runtime)?;

    if config.locations.is_empty() {
        return Err(Error::Config("at least one location must be configured".to_string()));
    }
    server::validate_server(&config.server)?;
    server::validate_listeners(&config.listeners, &config.server, &config.servers)?;
    let upstream_names = upstream::validate_upstreams(&config.upstreams)?;
    route::validate_locations(None, &config.locations, &upstream_names)?;

    let mut all_server_names = HashSet::new();
    server::validate_server_names("server", &config.server.server_names, &mut all_server_names)?;
    vhost::validate_virtual_hosts(&config.servers, &upstream_names, &mut all_server_names)?;

    Ok(())
}

#[cfg(test)]
mod tests;
