use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rginx_core::{Listener, Result, VirtualHostTls};

use crate::model::{ListenerConfig, ServerConfig, VirtualHostConfig, VirtualHostTlsConfig};

mod fields;
mod http3;
mod listener;
mod listener_managed_identity;
mod tls;

#[cfg(test)]
pub(super) const DEFAULT_HTTP3_MAX_CONCURRENT_STREAMS: usize =
    http3::DEFAULT_HTTP3_MAX_CONCURRENT_STREAMS;
#[cfg(test)]
pub(super) const DEFAULT_HTTP3_STREAM_BUFFER_SIZE_BYTES: usize =
    http3::DEFAULT_HTTP3_STREAM_BUFFER_SIZE_BYTES;
#[cfg(test)]
pub(super) const DEFAULT_HTTP3_ACTIVE_CONNECTION_ID_LIMIT: u32 =
    http3::DEFAULT_HTTP3_ACTIVE_CONNECTION_ID_LIMIT;
#[cfg(test)]
pub(super) const DEFAULT_HTTP3_RETRY: bool = http3::DEFAULT_HTTP3_RETRY;
#[cfg(test)]
pub(super) const DEFAULT_HTTP3_GSO: bool = http3::DEFAULT_HTTP3_GSO;

pub(super) struct CompiledServer {
    pub listener: Listener,
    pub server_names: Vec<String>,
}

/// Compiles the legacy top-level `server` block into the default listener model.
pub(super) fn compile_legacy_server(
    server: ServerConfig,
    base_dir: &Path,
    any_vhost_tls: bool,
    allow_missing_managed_tls_identity: bool,
    managed_identity_pairs: &HashSet<(PathBuf, PathBuf)>,
) -> Result<CompiledServer> {
    listener::compile_legacy_server(
        server,
        base_dir,
        any_vhost_tls,
        allow_missing_managed_tls_identity,
        managed_identity_pairs,
    )
}

/// Compiles explicit listener blocks into runtime listener definitions.
pub(super) fn compile_listeners(
    listeners: Vec<ListenerConfig>,
    default_server_header: Option<String>,
    base_dir: &Path,
    allow_missing_managed_tls_identity: bool,
    managed_identity_pairs: &HashSet<(PathBuf, PathBuf)>,
) -> Result<Vec<Listener>> {
    listener::compile_listeners(
        listeners,
        default_server_header,
        base_dir,
        allow_missing_managed_tls_identity,
        managed_identity_pairs,
    )
}

/// Compiles nginx-style `servers[].listen` entries into deduplicated runtime listeners.
pub(super) fn compile_vhost_listeners(
    vhosts: &[VirtualHostConfig],
    server_defaults: &ServerConfig,
    base_dir: &Path,
    allow_missing_managed_tls_identity: bool,
    managed_identity_pairs: &HashSet<(PathBuf, PathBuf)>,
) -> Result<Vec<Listener>> {
    listener::compile_vhost_listeners(
        vhosts,
        server_defaults,
        base_dir,
        allow_missing_managed_tls_identity,
        managed_identity_pairs,
    )
}

/// Compiles per-vhost TLS overrides into the runtime vhost TLS structure.
pub(super) fn compile_virtual_host_tls(
    tls: Option<VirtualHostTlsConfig>,
    base_dir: &Path,
    allow_missing_identity: bool,
) -> Result<Option<VirtualHostTls>> {
    tls::compile_virtual_host_tls(tls, base_dir, allow_missing_identity)
}
