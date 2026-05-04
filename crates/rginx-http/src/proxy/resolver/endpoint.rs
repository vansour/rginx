use std::net::{IpAddr, SocketAddr};

use hickory_resolver::TokioResolver;
use hickory_resolver::config::{
    ConnectionConfig, LookupIpStrategy, NameServerConfig, ResolverConfig, ResolverOpts,
};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use rginx_core::{Error, UpstreamDnsPolicy, UpstreamPeer};

use super::*;

pub(super) fn build_resolver(policy: &UpstreamDnsPolicy) -> Result<TokioResolver, Error> {
    let mut options = ResolverOpts::default();
    options.ip_strategy = if policy.prefer_ipv4 {
        LookupIpStrategy::Ipv4thenIpv6
    } else if policy.prefer_ipv6 {
        LookupIpStrategy::Ipv6thenIpv4
    } else {
        LookupIpStrategy::Ipv4AndIpv6
    };

    if policy.resolver_addrs.is_empty() {
        return TokioResolver::builder_tokio()
            .map_err(|error| {
                Error::Server(format!("failed to initialize system dns resolver: {error}"))
            })?
            .with_options(options)
            .build()
            .map_err(|error| {
                Error::Server(format!("failed to initialize system dns resolver: {error}"))
            });
    }

    let mut config = ResolverConfig::default();
    for socket_addr in &policy.resolver_addrs {
        let mut udp = ConnectionConfig::udp();
        udp.port = socket_addr.port();

        let mut tcp = ConnectionConfig::tcp();
        tcp.port = socket_addr.port();

        config.add_name_server(NameServerConfig::new(socket_addr.ip(), true, vec![udp, tcp]));
    }

    TokioResolver::builder_with_config(config, TokioRuntimeProvider::default())
        .with_options(options)
        .build()
        .map_err(|error| Error::Server(format!("failed to initialize dns resolver: {error}")))
}

pub(super) fn parse_peer_addressing(peer: &UpstreamPeer) -> Result<PeerAddressing, Error> {
    let uri = peer.url.parse::<http::Uri>().map_err(|error| {
        Error::Server(format!("failed to parse upstream peer url `{}`: {error}", peer.url))
    })?;
    let host = uri.host().ok_or_else(|| {
        Error::Server(format!("upstream peer `{}` is missing a hostname", peer.url))
    })?;
    let port = uri.port_u16().unwrap_or(if peer.scheme == "https" { 443 } else { 80 });

    Ok(PeerAddressing {
        host: host.to_string(),
        port,
        scheme: peer.scheme.clone(),
        authority: peer.authority.clone(),
        logical_peer_url: peer.url.clone(),
        weight: peer.weight,
        backup: peer.backup,
        max_conns: peer.max_conns,
    })
}

pub(super) fn build_endpoint(addressing: &PeerAddressing, ip: IpAddr) -> ResolvedUpstreamPeer {
    let dial_authority = socket_addr_authority(SocketAddr::new(ip, addressing.port));
    let endpoint_key = if addressing.authority == dial_authority {
        addressing.logical_peer_url.clone()
    } else {
        format!("{}|{}", addressing.logical_peer_url, dial_authority)
    };
    ResolvedUpstreamPeer {
        url: addressing.logical_peer_url.clone(),
        logical_peer_url: addressing.logical_peer_url.clone(),
        endpoint_key,
        display_url: format!("{}://{}", addressing.scheme, dial_authority),
        scheme: addressing.scheme.clone(),
        upstream_authority: addressing.authority.clone(),
        dial_authority: dial_authority.clone(),
        socket_addr: SocketAddr::new(ip, addressing.port),
        server_name: addressing.host.clone(),
        weight: addressing.weight,
        backup: addressing.backup,
        max_conns: addressing.max_conns,
    }
}

fn socket_addr_authority(socket_addr: SocketAddr) -> String {
    match socket_addr {
        SocketAddr::V4(addr) => addr.to_string(),
        SocketAddr::V6(addr) => format!("[{}]:{}", addr.ip(), addr.port()),
    }
}

pub(super) fn clamp_ttl(
    value: std::time::Duration,
    min: std::time::Duration,
    max: std::time::Duration,
) -> std::time::Duration {
    value.max(min).min(max)
}

pub(super) fn refresh_due(
    now: std::time::Instant,
    valid_until: std::time::Instant,
    refresh_before_expiry: std::time::Duration,
) -> bool {
    now >= valid_until || now + refresh_before_expiry >= valid_until
}

pub(super) fn order_addresses(
    mut addresses: Vec<IpAddr>,
    policy: &UpstreamDnsPolicy,
) -> Vec<IpAddr> {
    if policy.prefer_ipv4 {
        addresses.sort_by_key(|ip| (ip.is_ipv6(), *ip));
    } else if policy.prefer_ipv6 {
        addresses.sort_by_key(|ip| (ip.is_ipv4(), *ip));
    } else {
        addresses.sort();
    }
    addresses
}

pub(super) fn duration_to_ms(duration: std::time::Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}
