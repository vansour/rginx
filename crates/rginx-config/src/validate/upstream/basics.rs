use super::*;

pub(super) fn validate_upstream_name_and_peers(
    upstream: &UpstreamConfig,
    upstream_names: &mut HashSet<String>,
) -> Result<()> {
    if upstream.name.trim().is_empty() {
        return Err(Error::Config("upstream name must not be empty".to_string()));
    }

    if !upstream_names.insert(upstream.name.clone()) {
        return Err(Error::Config(format!("duplicate upstream `{}`", upstream.name)));
    }

    if upstream.peers.is_empty() {
        return Err(Error::Config(format!(
            "upstream `{}` must define at least one peer",
            upstream.name
        )));
    }

    for peer in &upstream.peers {
        if peer.url.trim().is_empty() {
            return Err(Error::Config(format!(
                "upstream `{}` contains an empty peer url",
                upstream.name
            )));
        }

        if peer.weight == 0 {
            return Err(Error::Config(format!(
                "upstream `{}` peer `{}` weight must be greater than 0",
                upstream.name, peer.url
            )));
        }

        if peer.max_conns.is_some_and(|max_conns| max_conns == 0) {
            return Err(Error::Config(format!(
                "upstream `{}` peer `{}` max_conns must be greater than 0",
                upstream.name, peer.url
            )));
        }
    }

    Ok(())
}

pub(super) fn validate_server_name_override(upstream: &UpstreamConfig) -> Result<()> {
    if let Some(server_name_override) = &upstream.server_name_override
        && server_name_override.trim().is_empty()
    {
        return Err(Error::Config(format!(
            "upstream `{}` server_name_override must not be empty",
            upstream.name
        )));
    }

    Ok(())
}
