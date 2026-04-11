use std::path::Path;

use crate::model::Config;
use rginx_core::{ConfigSnapshot, Result, VirtualHost};

use crate::validate::validate;

mod path;
mod route;
mod runtime;
mod server;
mod upstream;
mod vhost;

const DEFAULT_UPSTREAM_REQUEST_TIMEOUT_SECS: u64 = 30;
const DEFAULT_UPSTREAM_CONNECT_TIMEOUT_SECS: u64 = DEFAULT_UPSTREAM_REQUEST_TIMEOUT_SECS;
const DEFAULT_UPSTREAM_WRITE_TIMEOUT_SECS: u64 = DEFAULT_UPSTREAM_REQUEST_TIMEOUT_SECS;
const DEFAULT_UPSTREAM_IDLE_TIMEOUT_SECS: u64 = DEFAULT_UPSTREAM_REQUEST_TIMEOUT_SECS;
const DEFAULT_UPSTREAM_POOL_IDLE_TIMEOUT_SECS: u64 = 90;
const DEFAULT_UPSTREAM_POOL_MAX_IDLE_PER_HOST: usize = usize::MAX;
const DEFAULT_UPSTREAM_HTTP2_KEEP_ALIVE_TIMEOUT_SECS: u64 = 20;
const DEFAULT_MAX_REPLAYABLE_REQUEST_BODY_BYTES: u64 = 64 * 1024;
const DEFAULT_UNHEALTHY_AFTER_FAILURES: u32 = 2;
const DEFAULT_UNHEALTHY_COOLDOWN_SECS: u64 = 10;
const DEFAULT_HEALTH_CHECK_INTERVAL_SECS: u64 = 5;
const DEFAULT_HEALTH_CHECK_TIMEOUT_SECS: u64 = 2;
const DEFAULT_HEALTHY_SUCCESSES_REQUIRED: u32 = 2;
const DEFAULT_VHOST_ID: &str = "server";
const DEFAULT_GRPC_HEALTH_CHECK_PATH: &str = "/grpc.health.v1.Health/Check";

use path::resolve_path;

pub fn compile(raw: Config) -> Result<ConfigSnapshot> {
    compile_with_base(raw, Path::new("."))
}

pub fn compile_with_base(raw: Config, base_dir: impl AsRef<Path>) -> Result<ConfigSnapshot> {
    validate(&raw)?;
    let base_dir = base_dir.as_ref();

    let Config {
        runtime,
        listeners: raw_listeners,
        server,
        upstreams: raw_upstreams,
        locations,
        servers: raw_servers,
    } = raw;
    let runtime = runtime::compile_runtime_settings(runtime)?;
    let any_vhost_tls = raw_servers.iter().any(|vhost| vhost.tls.is_some());
    let (listeners, default_server_names) = if raw_listeners.is_empty() {
        let compiled_server = server::compile_legacy_server(server, base_dir, any_vhost_tls)?;
        (vec![compiled_server.listener.clone()], compiled_server.server_names)
    } else {
        let default_server_names = server.server_names;
        let listeners = server::compile_listeners(raw_listeners, base_dir)?;
        (listeners, default_server_names)
    };
    let upstreams = upstream::compile_upstreams(raw_upstreams, base_dir)?;

    let default_vhost = VirtualHost {
        id: DEFAULT_VHOST_ID.to_string(),
        server_names: default_server_names,
        routes: route::compile_routes(locations, &upstreams, DEFAULT_VHOST_ID)?,
        tls: None,
    };

    let vhosts = raw_servers
        .into_iter()
        .enumerate()
        .map(|(index, vhost_config)| {
            vhost::compile_virtual_host(
                format!("servers[{index}]"),
                vhost_config,
                &upstreams,
                base_dir,
            )
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(ConfigSnapshot { runtime, listeners, default_vhost, vhosts, upstreams })
}
#[cfg(test)]
mod tests;
