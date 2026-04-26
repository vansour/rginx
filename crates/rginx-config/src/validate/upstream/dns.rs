use super::*;

pub(super) fn validate_dns_settings(upstream: &UpstreamConfig) -> Result<()> {
    let Some(dns) = &upstream.dns else {
        return Ok(());
    };

    for resolver_addr in &dns.resolver_addrs {
        if resolver_addr.trim().is_empty() {
            return Err(Error::Config(format!(
                "upstream `{}` dns.resolver_addrs must not contain blank entries",
                upstream.name
            )));
        }
        resolver_addr.parse::<std::net::SocketAddr>().map_err(|error| {
            Error::Config(format!(
                "upstream `{}` dns.resolver_addrs entry `{resolver_addr}` is invalid: {error}",
                upstream.name
            ))
        })?;
    }

    for (field, value) in [
        ("dns.min_ttl_secs", dns.min_ttl_secs),
        ("dns.max_ttl_secs", dns.max_ttl_secs),
        ("dns.negative_ttl_secs", dns.negative_ttl_secs),
        ("dns.stale_if_error_secs", dns.stale_if_error_secs),
        ("dns.refresh_before_expiry_secs", dns.refresh_before_expiry_secs),
    ] {
        if value.is_some_and(|value| value == 0) {
            return Err(Error::Config(format!(
                "upstream `{}` {field} must be greater than 0",
                upstream.name
            )));
        }
    }

    if dns.prefer_ipv4.unwrap_or(false) && dns.prefer_ipv6.unwrap_or(false) {
        return Err(Error::Config(format!(
            "upstream `{}` dns.prefer_ipv4 and dns.prefer_ipv6 cannot both be true",
            upstream.name
        )));
    }

    if let (Some(min_ttl), Some(max_ttl)) = (dns.min_ttl_secs, dns.max_ttl_secs)
        && min_ttl > max_ttl
    {
        return Err(Error::Config(format!(
            "upstream `{}` dns.min_ttl_secs must be less than or equal to dns.max_ttl_secs",
            upstream.name
        )));
    }

    Ok(())
}
