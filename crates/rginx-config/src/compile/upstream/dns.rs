use std::net::SocketAddr;

use rginx_core::{Error, Result, UpstreamDnsPolicy};

use crate::model::UpstreamDnsConfig;

use super::settings::compile_timeout_secs;

pub(super) fn compile_dns_policy(
    upstream_name: &str,
    dns: Option<UpstreamDnsConfig>,
) -> Result<UpstreamDnsPolicy> {
    let Some(dns) = dns else {
        return Ok(UpstreamDnsPolicy::default());
    };

    let resolver_addrs = dns
        .resolver_addrs
        .into_iter()
        .map(|value| {
            value.parse::<SocketAddr>().map_err(|error| {
                Error::Config(format!(
                    "upstream `{upstream_name}` dns.resolver_addrs entry `{value}` is invalid: {error}"
                ))
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let min_ttl = compile_timeout_secs(
        dns.min_ttl_secs.unwrap_or(super::super::DEFAULT_UPSTREAM_DNS_MIN_TTL_SECS),
        upstream_name,
        "dns.min_ttl_secs",
    )?;
    let max_ttl = compile_timeout_secs(
        dns.max_ttl_secs.unwrap_or(super::super::DEFAULT_UPSTREAM_DNS_MAX_TTL_SECS),
        upstream_name,
        "dns.max_ttl_secs",
    )?;
    let negative_ttl = compile_timeout_secs(
        dns.negative_ttl_secs.unwrap_or(super::super::DEFAULT_UPSTREAM_DNS_NEGATIVE_TTL_SECS),
        upstream_name,
        "dns.negative_ttl_secs",
    )?;
    let stale_if_error = compile_timeout_secs(
        dns.stale_if_error_secs.unwrap_or(super::super::DEFAULT_UPSTREAM_DNS_STALE_IF_ERROR_SECS),
        upstream_name,
        "dns.stale_if_error_secs",
    )?;
    let refresh_before_expiry = compile_timeout_secs(
        dns.refresh_before_expiry_secs
            .unwrap_or(super::super::DEFAULT_UPSTREAM_DNS_REFRESH_BEFORE_EXPIRY_SECS),
        upstream_name,
        "dns.refresh_before_expiry_secs",
    )?;

    if min_ttl > max_ttl {
        return Err(Error::Config(format!(
            "upstream `{upstream_name}` dns.min_ttl_secs must be less than or equal to dns.max_ttl_secs"
        )));
    }

    Ok(UpstreamDnsPolicy {
        resolver_addrs,
        min_ttl,
        max_ttl,
        negative_ttl,
        stale_if_error,
        refresh_before_expiry,
        prefer_ipv4: dns.prefer_ipv4.unwrap_or(false),
        prefer_ipv6: dns.prefer_ipv6.unwrap_or(false),
    })
}
