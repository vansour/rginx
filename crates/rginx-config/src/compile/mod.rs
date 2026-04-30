use std::path::Path;
use std::{collections::HashSet, path::PathBuf};

use crate::model::Config;
use crate::model::VirtualHostConfig;
use rginx_core::{ConfigSnapshot, Result, VirtualHost};

use crate::validate::validate;

mod acme;
mod cache;
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
const DEFAULT_UPSTREAM_DNS_MIN_TTL_SECS: u64 = 5;
const DEFAULT_UPSTREAM_DNS_MAX_TTL_SECS: u64 = 300;
const DEFAULT_UPSTREAM_DNS_NEGATIVE_TTL_SECS: u64 = 30;
const DEFAULT_UPSTREAM_DNS_STALE_IF_ERROR_SECS: u64 = 60;
const DEFAULT_UPSTREAM_DNS_REFRESH_BEFORE_EXPIRY_SECS: u64 = 10;
const DEFAULT_VHOST_ID: &str = "server";
const DEFAULT_GRPC_HEALTH_CHECK_PATH: &str = "/grpc.health.v1.Health/Check";

use path::resolve_path;

#[derive(Debug, Clone, Copy, Default)]
pub struct CompileOptions {
    pub allow_missing_managed_tls_identity: bool,
}

pub fn compile(raw: Config) -> Result<ConfigSnapshot> {
    compile_with_base(raw, Path::new("."))
}

pub fn compile_with_base(raw: Config, base_dir: impl AsRef<Path>) -> Result<ConfigSnapshot> {
    compile_with_base_and_options(raw, base_dir, CompileOptions::default())
}

pub fn compile_with_base_and_options(
    raw: Config,
    base_dir: impl AsRef<Path>,
    options: CompileOptions,
) -> Result<ConfigSnapshot> {
    validate(&raw)?;
    let base_dir = base_dir.as_ref();
    let managed_identity_pairs = collect_managed_identity_pairs(&raw.servers, base_dir);

    let Config {
        runtime,
        acme: raw_acme,
        listeners: raw_listeners,
        cache_zones: raw_cache_zones,
        server,
        upstreams: raw_upstreams,
        locations,
        servers: raw_servers,
    } = raw;
    let runtime = runtime::compile_runtime_settings(runtime)?;
    let acme = acme::compile_global_acme(raw_acme, base_dir);
    let cache_zones = cache::compile_cache_zones(raw_cache_zones, base_dir)?;
    let any_vhost_tls = raw_servers.iter().any(|vhost| vhost.tls.is_some());
    let any_vhost_listen = raw_servers.iter().any(|vhost| !vhost.listen.is_empty());
    let (listeners, default_server_names) = if any_vhost_listen {
        let listeners = server::compile_vhost_listeners(
            &raw_servers,
            &server,
            base_dir,
            options.allow_missing_managed_tls_identity,
            &managed_identity_pairs,
        )?;
        (listeners, server.server_names.clone())
    } else if raw_listeners.is_empty() {
        let compiled_server = server::compile_legacy_server(
            server,
            base_dir,
            any_vhost_tls,
            options.allow_missing_managed_tls_identity,
            &managed_identity_pairs,
        )?;
        (vec![compiled_server.listener.clone()], compiled_server.server_names)
    } else {
        let default_server_header = server.server_header;
        let default_server_names = server.server_names;
        let listeners = server::compile_listeners(
            raw_listeners,
            default_server_header,
            base_dir,
            options.allow_missing_managed_tls_identity,
            &managed_identity_pairs,
        )?;
        (listeners, default_server_names)
    };
    let mut upstreams = upstream::compile_upstreams(raw_upstreams, base_dir)?;

    let default_vhost = VirtualHost {
        id: DEFAULT_VHOST_ID.to_string(),
        server_names: default_server_names,
        routes: route::compile_routes(locations, &upstreams, DEFAULT_VHOST_ID)?,
        tls: None,
    };

    let mut managed_certificates = Vec::new();
    let compiled_vhosts = raw_servers
        .into_iter()
        .enumerate()
        .map(|(index, vhost_config)| {
            vhost::compile_virtual_host(
                format!("servers[{index}]"),
                vhost_config,
                &upstreams,
                base_dir,
                options.allow_missing_managed_tls_identity,
                &managed_identity_pairs,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let mut vhosts = Vec::with_capacity(compiled_vhosts.len());
    for compiled in compiled_vhosts {
        upstreams.extend(compiled.upstreams);
        if let Some(spec) = compiled.managed_certificate {
            managed_certificates.push(spec);
        }
        vhosts.push(compiled.vhost);
    }

    Ok(ConfigSnapshot {
        runtime,
        acme,
        managed_certificates,
        listeners,
        default_vhost,
        vhosts,
        cache_zones,
        upstreams,
    })
}

fn collect_managed_identity_pairs(
    raw_servers: &[VirtualHostConfig],
    base_dir: &Path,
) -> HashSet<(PathBuf, PathBuf)> {
    raw_servers
        .iter()
        .filter_map(|vhost| {
            let tls = vhost.tls.as_ref()?;
            tls.acme.as_ref()?;
            Some((
                resolve_path(base_dir, tls.cert_path.clone()),
                resolve_path(base_dir, tls.key_path.clone()),
            ))
        })
        .collect()
}
#[cfg(test)]
mod tests;
