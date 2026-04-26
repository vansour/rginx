use rginx_core::Result;

use crate::model::{ListenerConfig, ServerConfig, VirtualHostConfig};

mod http3;
mod listener;
mod names;
mod proxies;
mod tls;

/// Validates the legacy top-level `server` block.
pub(super) fn validate_server(server: &ServerConfig) -> Result<()> {
    listener::validate_server(server)
}

/// Validates the explicit listener list and its interaction with the legacy server block.
pub(super) fn validate_listeners(
    listeners: &[ListenerConfig],
    server: &ServerConfig,
    vhosts: &[VirtualHostConfig],
) -> Result<()> {
    listener::validate_listeners(listeners, server, vhosts)
}

pub(super) fn validate_server_names(
    owner_label: &str,
    server_names: &[String],
    all_server_names: &mut std::collections::HashSet<String>,
) -> Result<()> {
    names::validate_server_names(owner_label, server_names, all_server_names)
}

pub(super) fn validate_tls_identity_fields(
    owner_label: &str,
    cert_path: &str,
    key_path: &str,
    additional_certificates: Option<&[crate::model::ServerCertificateBundleConfig]>,
    ocsp_staple_path: Option<&str>,
) -> Result<()> {
    tls::validate_tls_identity_fields(
        owner_label,
        cert_path,
        key_path,
        additional_certificates,
        ocsp_staple_path,
    )
}
