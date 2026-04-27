use std::collections::HashSet;

use rginx_core::{Error, Result};

use crate::model::{ServerTlsConfig, VirtualHostConfig, VirtualHostTlsConfig};

pub(super) fn validate_virtual_hosts(
    vhosts: &[VirtualHostConfig],
    global_upstream_names: &HashSet<String>,
    all_server_names: &mut HashSet<String>,
    require_vhost_listen: bool,
    server_tls_defaults: Option<&ServerTlsConfig>,
) -> Result<()> {
    for (idx, vhost) in vhosts.iter().enumerate() {
        let vhost_label = format!("servers[{idx}]");

        validate_vhost_listens(&vhost_label, vhost, require_vhost_listen, server_tls_defaults)?;

        if vhost.server_names.is_empty() {
            if vhost.tls.is_some() {
                return Err(Error::Config(format!(
                    "{vhost_label} TLS requires at least one server_name"
                )));
            }

            return Err(Error::Config(format!(
                "{vhost_label} must define at least one server_name"
            )));
        }

        super::server::validate_server_names(&vhost_label, &vhost.server_names, all_server_names)?;

        if vhost.locations.is_empty() {
            return Err(Error::Config(format!("{vhost_label} must have at least one location")));
        }

        if let Some(tls) = &vhost.tls {
            super::server::validate_tls_identity_fields(
                &vhost_label,
                &tls.cert_path,
                &tls.key_path,
                tls.additional_certificates.as_deref(),
                tls.ocsp_staple_path.as_deref(),
            )?;
        }

        let local_upstream_names = super::upstream::validate_upstreams(&vhost.upstreams)?;
        let visible_upstream_names =
            visible_upstream_names(global_upstream_names, &local_upstream_names);
        super::route::validate_locations(
            Some(&vhost_label),
            &vhost.locations,
            &visible_upstream_names,
        )?;
    }

    Ok(())
}

fn validate_vhost_listens(
    vhost_label: &str,
    vhost: &VirtualHostConfig,
    require_vhost_listen: bool,
    server_tls_defaults: Option<&ServerTlsConfig>,
) -> Result<()> {
    if require_vhost_listen && vhost.listen.is_empty() {
        return Err(Error::Config(format!(
            "{vhost_label} must define at least one listen when servers[].listen is used; once any vhost uses servers[].listen, every vhost must declare listen explicitly"
        )));
    }

    let mut has_ssl_listen = false;
    let mut has_http3_listen = false;
    for (index, listen) in vhost.listen.iter().enumerate() {
        let owner = format!("{vhost_label}.listen[{index}]");
        let parsed = crate::listen::parse_vhost_listen(&owner, listen)?;
        has_ssl_listen |= parsed.ssl;
        has_http3_listen |= parsed.http3;
    }

    if has_ssl_listen && vhost.tls.is_none() {
        return Err(Error::Config(format!("{vhost_label} ssl listen requires tls")));
    }

    if require_vhost_listen && vhost.tls.is_some() && !has_ssl_listen {
        return Err(Error::Config(format!("{vhost_label} TLS requires an ssl listen")));
    }

    if vhost.http3.is_some() && !has_http3_listen {
        return Err(Error::Config(format!("{vhost_label} http3 requires an http3 listen")));
    }

    if let Some(http3) = vhost.http3.as_ref() {
        let vhost_tls_policy = vhost.tls.as_ref().map(server_tls_policy_from_vhost_tls);
        let tls_policy = server_tls_defaults.or(vhost_tls_policy.as_ref());
        super::server::validate_http3_config(&format!("{vhost_label}.http3"), http3, tls_policy)?;
    }

    Ok(())
}

fn server_tls_policy_from_vhost_tls(tls: &VirtualHostTlsConfig) -> ServerTlsConfig {
    ServerTlsConfig {
        cert_path: tls.cert_path.clone(),
        key_path: tls.key_path.clone(),
        additional_certificates: tls.additional_certificates.clone(),
        versions: None,
        cipher_suites: None,
        key_exchange_groups: None,
        alpn_protocols: None,
        ocsp_staple_path: tls.ocsp_staple_path.clone(),
        ocsp: tls.ocsp.clone(),
        session_resumption: None,
        session_tickets: None,
        session_cache_size: None,
        session_ticket_count: None,
        client_auth: None,
    }
}

fn visible_upstream_names(
    global_names: &HashSet<String>,
    local_names: &HashSet<String>,
) -> HashSet<String> {
    global_names.union(local_names).cloned().collect()
}
