use std::net::IpAddr;

use ipnet::IpNet;
use rginx_core::{Error, Result};

/// Validates a trusted proxy entry and annotates any error with its owner path.
pub(super) fn validate_trusted_proxy_with_owner(owner_label: &str, value: &str) -> Result<()> {
    let normalized = normalize_trusted_proxy(value).ok_or_else(|| {
        Error::Config(format!(
            "{owner_label} trusted_proxies entry `{value}` must be a valid IP address or CIDR"
        ))
    })?;

    normalized.parse::<IpNet>().map_err(|error| {
        Error::Config(format!("{owner_label} trusted_proxies entry `{value}` is invalid: {error}"))
    })?;

    Ok(())
}

/// Normalizes a trusted proxy entry into canonical IP or CIDR form.
pub(super) fn normalize_trusted_proxy(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.contains('/') {
        return Some(trimmed.to_string());
    }

    let ip = trimmed.parse::<IpAddr>().ok()?;
    Some(match ip {
        IpAddr::V4(_) => format!("{trimmed}/32"),
        IpAddr::V6(_) => format!("{trimmed}/128"),
    })
}
