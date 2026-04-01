use std::collections::HashSet;

use rginx_core::{Error, Result};

use crate::model::VirtualHostConfig;

pub(super) fn validate_virtual_hosts(
    vhosts: &[VirtualHostConfig],
    upstream_names: &HashSet<String>,
    all_server_names: &mut HashSet<String>,
    config_api_token: Option<&str>,
) -> Result<()> {
    for (idx, vhost) in vhosts.iter().enumerate() {
        let vhost_label = format!("servers[{idx}]");

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
            if tls.cert_path.trim().is_empty() {
                return Err(Error::Config(format!(
                    "{vhost_label} TLS certificate path must not be empty"
                )));
            }

            if tls.key_path.trim().is_empty() {
                return Err(Error::Config(format!(
                    "{vhost_label} TLS private key path must not be empty"
                )));
            }
        }

        super::route::validate_locations(
            Some(&vhost_label),
            &vhost.locations,
            upstream_names,
            config_api_token,
        )?;
    }

    Ok(())
}
