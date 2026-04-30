use std::collections::BTreeSet;

use rginx_core::{Error, Result};

use crate::model::{
    AcmeConfig, Config, VirtualHostAcmeConfig, VirtualHostConfig, VirtualHostTlsConfig,
};

pub(super) fn validate_acme(config: &Config) -> Result<()> {
    if let Some(acme) = config.acme.as_ref() {
        validate_global_acme(acme)?;
    }

    let mut any_managed_vhost = false;
    for (index, vhost) in config.servers.iter().enumerate() {
        let Some(tls) = vhost.tls.as_ref() else {
            continue;
        };
        let Some(acme) = tls.acme.as_ref() else {
            continue;
        };

        any_managed_vhost = true;
        validate_vhost_acme(&format!("servers[{index}]"), vhost, tls, acme, config.acme.as_ref())?;
    }

    if any_managed_vhost && !has_http01_listener(config)? {
        return Err(Error::Config(
            "ACME HTTP-01 requires at least one plain HTTP listener bound to port 80".to_string(),
        ));
    }

    Ok(())
}

fn validate_global_acme(acme: &AcmeConfig) -> Result<()> {
    if acme.directory_url.trim().is_empty() {
        return Err(Error::Config("acme.directory_url must not be empty".to_string()));
    }

    if acme.state_dir.trim().is_empty() {
        return Err(Error::Config("acme.state_dir must not be empty".to_string()));
    }

    for (index, contact) in acme.contacts.iter().enumerate() {
        if contact.trim().is_empty() {
            return Err(Error::Config(format!("acme.contacts[{index}] must not be empty")));
        }
    }

    if acme.renew_before_days.is_some_and(|days| days == 0) {
        return Err(Error::Config("acme.renew_before_days must be greater than 0".to_string()));
    }

    if acme.poll_interval_secs.is_some_and(|secs| secs == 0) {
        return Err(Error::Config("acme.poll_interval_secs must be greater than 0".to_string()));
    }

    Ok(())
}

fn validate_vhost_acme(
    owner_label: &str,
    vhost: &VirtualHostConfig,
    tls: &VirtualHostTlsConfig,
    acme: &VirtualHostAcmeConfig,
    global_acme: Option<&AcmeConfig>,
) -> Result<()> {
    if global_acme.is_none() {
        return Err(Error::Config(format!(
            "{owner_label} TLS ACME requires top-level acme configuration"
        )));
    }

    if tls.additional_certificates.as_ref().is_some_and(|bundles| !bundles.is_empty()) {
        return Err(Error::Config(format!(
            "{owner_label} TLS ACME does not support additional_certificates in phase 1"
        )));
    }

    if acme.domains.is_empty() {
        return Err(Error::Config(format!("{owner_label} TLS ACME domains must not be empty")));
    }

    let normalized_server_names = normalize_unique_domains(
        &vhost.server_names,
        &format!("{owner_label} server_names"),
        true,
    )?;
    let normalized_domains =
        normalize_unique_domains(&acme.domains, &format!("{owner_label} TLS ACME domains"), true)?;

    if normalized_domains != normalized_server_names {
        return Err(Error::Config(format!(
            "{owner_label} TLS ACME domains must match server_names exactly in phase 1"
        )));
    }

    if acme
        .challenge
        .is_some_and(|challenge| !matches!(challenge, crate::model::AcmeChallengeConfig::Http01))
    {
        return Err(Error::Config(format!(
            "{owner_label} TLS ACME only supports HTTP-01 in phase 1"
        )));
    }

    Ok(())
}

fn normalize_unique_domains(
    values: &[String],
    owner_label: &str,
    reject_wildcards: bool,
) -> Result<BTreeSet<String>> {
    let mut normalized = BTreeSet::new();

    for (index, value) in values.iter().enumerate() {
        let value = value.trim();
        if value.is_empty() {
            return Err(Error::Config(format!("{owner_label}[{index}] must not be empty")));
        }

        if reject_wildcards && value.contains('*') {
            return Err(Error::Config(format!(
                "{owner_label}[{index}] wildcard `{value}` is not supported by ACME phase 1"
            )));
        }

        let lowered = value.to_ascii_lowercase();
        if !normalized.insert(lowered) {
            return Err(Error::Config(format!(
                "{owner_label}[{index}] duplicates another ACME domain entry"
            )));
        }
    }

    Ok(normalized)
}

fn has_http01_listener(config: &Config) -> Result<bool> {
    let any_vhost_listen = config.servers.iter().any(|vhost| !vhost.listen.is_empty());
    if any_vhost_listen {
        for (vhost_index, vhost) in config.servers.iter().enumerate() {
            for (listen_index, listen) in vhost.listen.iter().enumerate() {
                let owner = format!("servers[{vhost_index}].listen[{listen_index}]");
                let parsed = crate::listen::parse_vhost_listen(&owner, listen)?;
                if parsed.addr.port() == 80 && !parsed.ssl {
                    return Ok(true);
                }
            }
        }
        return Ok(false);
    }

    if !config.listeners.is_empty() {
        for (listener_index, listener) in config.listeners.iter().enumerate() {
            if listener.tls.is_some() {
                continue;
            }

            let owner = format!("listeners[{listener_index}].listen");
            let listen_addr = listener.listen.parse::<std::net::SocketAddr>().map_err(|error| {
                Error::Config(format!("{owner} `{}` is invalid: {error}", listener.listen))
            })?;
            if listen_addr.port() == 80 {
                return Ok(true);
            }
        }

        return Ok(false);
    }

    // In legacy server.listen mode, any managed vhost certificate turns the single generated
    // listener into a TLS termination point for that bind, so it cannot satisfy HTTP-01's
    // requirement for a plain HTTP listener on port 80.
    Ok(false)
}
