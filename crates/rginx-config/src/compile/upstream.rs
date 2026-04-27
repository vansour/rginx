use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rginx_core::{Error, Result, Upstream, UpstreamSettings};

use crate::model::UpstreamConfig;

mod dns;
mod peer;
mod settings;
mod tls;

pub(super) fn compile_upstreams(
    raw_upstreams: Vec<UpstreamConfig>,
    base_dir: &Path,
) -> Result<HashMap<String, Arc<Upstream>>> {
    compile_upstreams_with_names(raw_upstreams, base_dir, |name| name.to_string())
}

pub(super) fn compile_scoped_upstreams(
    raw_upstreams: Vec<UpstreamConfig>,
    base_dir: &Path,
    scope: &str,
) -> Result<HashMap<String, Arc<Upstream>>> {
    compile_upstreams_with_names(raw_upstreams, base_dir, |name| scoped_upstream_name(scope, name))
}

pub(super) fn scoped_upstream_name(scope: &str, name: &str) -> String {
    format!("{scope}::{name}")
}

fn compile_upstreams_with_names(
    raw_upstreams: Vec<UpstreamConfig>,
    base_dir: &Path,
    name_mapper: impl Fn(&str) -> String,
) -> Result<HashMap<String, Arc<Upstream>>> {
    let compiled = raw_upstreams
        .into_iter()
        .map(|upstream| {
            let UpstreamConfig {
                name,
                peers,
                tls,
                dns,
                protocol,
                load_balance,
                server_name,
                server_name_override,
                request_timeout_secs,
                connect_timeout_secs,
                read_timeout_secs,
                write_timeout_secs,
                idle_timeout_secs,
                pool_idle_timeout_secs,
                pool_max_idle_per_host,
                tcp_keepalive_secs,
                tcp_nodelay,
                http2_keep_alive_interval_secs,
                http2_keep_alive_timeout_secs,
                http2_keep_alive_while_idle,
                max_replayable_request_body_bytes,
                unhealthy_after_failures,
                unhealthy_cooldown_secs,
                health_check_path,
                health_check_grpc_service,
                health_check_interval_secs,
                health_check_timeout_secs,
                healthy_successes_required,
            } = upstream;
            let name = name_mapper(&name);

            let peers = peers
                .into_iter()
                .map(|peer| peer::compile_peer(&name, peer.url, peer.weight, peer.backup))
                .collect::<Result<Vec<_>>>()?;
            let compiled_tls = tls::compile_tls(&name, tls, base_dir)?;
            let protocol = settings::compile_protocol(&name, protocol, &peers)?;
            let load_balance = settings::compile_load_balance(load_balance);
            let dns = dns::compile_dns_policy(&name, dns)?;
            let server_name_override =
                settings::compile_server_name_override(&name, server_name_override)?;
            let request_timeout = settings::compile_timeout_secs(
                read_timeout_secs
                    .or(request_timeout_secs)
                    .unwrap_or(super::DEFAULT_UPSTREAM_REQUEST_TIMEOUT_SECS),
                &name,
                "read_timeout_secs",
            )?;
            let connect_timeout = settings::compile_timeout_secs(
                connect_timeout_secs
                    .or(request_timeout_secs)
                    .unwrap_or(super::DEFAULT_UPSTREAM_CONNECT_TIMEOUT_SECS),
                &name,
                "connect_timeout_secs",
            )?;
            let write_timeout = settings::compile_timeout_secs(
                write_timeout_secs
                    .or(request_timeout_secs)
                    .unwrap_or(super::DEFAULT_UPSTREAM_WRITE_TIMEOUT_SECS),
                &name,
                "write_timeout_secs",
            )?;
            let idle_timeout = settings::compile_timeout_secs(
                idle_timeout_secs
                    .or(request_timeout_secs)
                    .unwrap_or(super::DEFAULT_UPSTREAM_IDLE_TIMEOUT_SECS),
                &name,
                "idle_timeout_secs",
            )?;
            let pool_idle_timeout = match pool_idle_timeout_secs {
                Some(0) => None,
                Some(timeout) => {
                    Some(settings::compile_timeout_secs(timeout, &name, "pool_idle_timeout_secs")?)
                }
                None => Some(Duration::from_secs(super::DEFAULT_UPSTREAM_POOL_IDLE_TIMEOUT_SECS)),
            };
            let pool_max_idle_per_host = settings::compile_pool_max_idle_per_host(
                &name,
                pool_max_idle_per_host
                    .unwrap_or(super::DEFAULT_UPSTREAM_POOL_MAX_IDLE_PER_HOST as u64),
            )?;
            let tcp_keepalive = tcp_keepalive_secs
                .map(|timeout| settings::compile_timeout_secs(timeout, &name, "tcp_keepalive_secs"))
                .transpose()?;
            let tcp_nodelay = tcp_nodelay.unwrap_or(false);
            let http2_keep_alive_interval = http2_keep_alive_interval_secs
                .map(|timeout| {
                    settings::compile_timeout_secs(timeout, &name, "http2_keep_alive_interval_secs")
                })
                .transpose()?;
            let http2_keep_alive_timeout = settings::compile_timeout_secs(
                http2_keep_alive_timeout_secs
                    .unwrap_or(super::DEFAULT_UPSTREAM_HTTP2_KEEP_ALIVE_TIMEOUT_SECS),
                &name,
                "http2_keep_alive_timeout_secs",
            )?;
            let http2_keep_alive_while_idle = http2_keep_alive_while_idle.unwrap_or(false);
            let max_replayable_request_body_bytes =
                settings::compile_max_replayable_request_body_bytes(
                    &name,
                    max_replayable_request_body_bytes,
                )?;
            let unhealthy_after_failures =
                unhealthy_after_failures.unwrap_or(super::DEFAULT_UNHEALTHY_AFTER_FAILURES);
            let unhealthy_cooldown = Duration::from_secs(
                unhealthy_cooldown_secs.unwrap_or(super::DEFAULT_UNHEALTHY_COOLDOWN_SECS),
            );
            let active_health_check = settings::compile_active_health_check(
                &name,
                health_check_path,
                health_check_grpc_service,
                health_check_interval_secs,
                health_check_timeout_secs,
                healthy_successes_required,
            )?;

            let compiled = Arc::new(Upstream::new(
                name.clone(),
                peers,
                compiled_tls.verify_mode,
                UpstreamSettings {
                    protocol,
                    load_balance,
                    dns,
                    server_name: server_name.unwrap_or(true),
                    server_name_override,
                    tls_versions: compiled_tls.tls_versions,
                    server_verify_depth: compiled_tls.server_verify_depth,
                    server_crl_path: compiled_tls.server_crl_path,
                    client_identity: compiled_tls.client_identity,
                    request_timeout,
                    connect_timeout,
                    write_timeout,
                    idle_timeout,
                    pool_idle_timeout,
                    pool_max_idle_per_host,
                    tcp_keepalive,
                    tcp_nodelay,
                    http2_keep_alive_interval,
                    http2_keep_alive_timeout,
                    http2_keep_alive_while_idle,
                    max_replayable_request_body_bytes,
                    unhealthy_after_failures,
                    unhealthy_cooldown,
                    active_health_check,
                },
            ));
            Ok((name, compiled))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut upstreams = HashMap::with_capacity(compiled.len());
    for (name, upstream) in compiled {
        if upstreams.insert(name.clone(), upstream).is_some() {
            return Err(Error::Config(format!("duplicate compiled upstream `{name}`")));
        }
    }

    Ok(upstreams)
}
