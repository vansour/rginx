use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use hickory_resolver::TokioAsyncResolver;
use hickory_resolver::config::{
    LookupIpStrategy, NameServerConfig, Protocol, ResolverConfig, ResolverOpts,
};
use rginx_core::{Error, UpstreamDnsPolicy, UpstreamPeer};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub(crate) struct ResolvedUpstreamPeer {
    #[cfg_attr(not(test), allow(dead_code))]
    pub url: String,
    pub logical_peer_url: String,
    pub endpoint_key: String,
    pub display_url: String,
    pub scheme: String,
    pub upstream_authority: String,
    pub dial_authority: String,
    pub socket_addr: SocketAddr,
    pub server_name: String,
    pub weight: u32,
    pub backup: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UpstreamResolverCacheEntrySnapshot {
    pub hostname: String,
    pub addresses: Vec<String>,
    pub negative: bool,
    pub valid_for_ms: Option<u64>,
    pub stale_for_ms: Option<u64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UpstreamResolverRuntimeSnapshot {
    pub resolve_requests_total: u64,
    pub cache_hits_total: u64,
    pub cache_misses_total: u64,
    pub refreshes_total: u64,
    pub resolve_errors_total: u64,
    pub stale_answers_total: u64,
    pub cache_entries: Vec<UpstreamResolverCacheEntrySnapshot>,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    addresses: Vec<IpAddr>,
    valid_until: Instant,
    stale_until: Instant,
    negative: bool,
    last_error: Option<String>,
}

#[derive(Debug, Clone)]
struct PeerAddressing {
    host: String,
    port: u16,
    scheme: String,
    authority: String,
    logical_peer_url: String,
    weight: u32,
    backup: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct UpstreamResolver {
    policy: UpstreamDnsPolicy,
    resolver: TokioAsyncResolver,
    cache: Arc<Mutex<HashMap<String, CacheEntry>>>,
    resolve_requests_total: Arc<AtomicU64>,
    cache_hits_total: Arc<AtomicU64>,
    cache_misses_total: Arc<AtomicU64>,
    refreshes_total: Arc<AtomicU64>,
    resolve_errors_total: Arc<AtomicU64>,
    stale_answers_total: Arc<AtomicU64>,
}

impl UpstreamResolver {
    pub(crate) fn new(policy: UpstreamDnsPolicy) -> Result<Self, Error> {
        let resolver = build_resolver(&policy)?;
        Ok(Self {
            policy,
            resolver,
            cache: Arc::new(Mutex::new(HashMap::new())),
            resolve_requests_total: Arc::new(AtomicU64::new(0)),
            cache_hits_total: Arc::new(AtomicU64::new(0)),
            cache_misses_total: Arc::new(AtomicU64::new(0)),
            refreshes_total: Arc::new(AtomicU64::new(0)),
            resolve_errors_total: Arc::new(AtomicU64::new(0)),
            stale_answers_total: Arc::new(AtomicU64::new(0)),
        })
    }

    pub(crate) async fn resolve_peer(
        &self,
        peer: &UpstreamPeer,
    ) -> Result<Vec<ResolvedUpstreamPeer>, Error> {
        let addressing = parse_peer_addressing(peer)?;
        if let Ok(ip) = IpAddr::from_str(&addressing.host) {
            return Ok(vec![build_endpoint(&addressing, ip)]);
        }

        let addresses = self.resolve_host(&addressing.host).await?;
        Ok(addresses.into_iter().map(|ip| build_endpoint(&addressing, ip)).collect())
    }

    pub(crate) async fn cached_peer_endpoints(
        &self,
        peer: &UpstreamPeer,
    ) -> Result<Vec<ResolvedUpstreamPeer>, Error> {
        let addressing = parse_peer_addressing(peer)?;
        if let Ok(ip) = IpAddr::from_str(&addressing.host) {
            return Ok(vec![build_endpoint(&addressing, ip)]);
        }

        let now = Instant::now();
        let cache = self.cache.lock().await;
        let Some(entry) = cache.get(&addressing.host) else {
            return Ok(Vec::new());
        };
        if entry.negative || entry.addresses.is_empty() || now > entry.stale_until {
            return Ok(Vec::new());
        }

        Ok(entry.addresses.iter().copied().map(|ip| build_endpoint(&addressing, ip)).collect())
    }

    pub(crate) async fn snapshot(&self) -> UpstreamResolverRuntimeSnapshot {
        let now = Instant::now();
        let cache = self.cache.lock().await;
        let mut cache_entries = cache
            .iter()
            .map(|(hostname, entry)| UpstreamResolverCacheEntrySnapshot {
                hostname: hostname.clone(),
                addresses: entry.addresses.iter().map(ToString::to_string).collect(),
                negative: entry.negative,
                valid_for_ms: entry.valid_until.checked_duration_since(now).map(duration_to_ms),
                stale_for_ms: entry.stale_until.checked_duration_since(now).map(duration_to_ms),
                last_error: entry.last_error.clone(),
            })
            .collect::<Vec<_>>();
        cache_entries.sort_by(|left, right| left.hostname.cmp(&right.hostname));

        UpstreamResolverRuntimeSnapshot {
            resolve_requests_total: self.resolve_requests_total.load(Ordering::Relaxed),
            cache_hits_total: self.cache_hits_total.load(Ordering::Relaxed),
            cache_misses_total: self.cache_misses_total.load(Ordering::Relaxed),
            refreshes_total: self.refreshes_total.load(Ordering::Relaxed),
            resolve_errors_total: self.resolve_errors_total.load(Ordering::Relaxed),
            stale_answers_total: self.stale_answers_total.load(Ordering::Relaxed),
            cache_entries,
        }
    }

    async fn resolve_host(&self, host: &str) -> Result<Vec<IpAddr>, Error> {
        self.resolve_requests_total.fetch_add(1, Ordering::Relaxed);
        let now = Instant::now();
        {
            let cache = self.cache.lock().await;
            if let Some(entry) = cache.get(host)
                && !entry.negative
                && !entry.addresses.is_empty()
                && !refresh_due(now, entry.valid_until, self.policy.refresh_before_expiry)
            {
                self.cache_hits_total.fetch_add(1, Ordering::Relaxed);
                return Ok(order_addresses(entry.addresses.clone(), &self.policy));
            }
        }

        self.cache_misses_total.fetch_add(1, Ordering::Relaxed);
        self.refreshes_total.fetch_add(1, Ordering::Relaxed);
        match self.lookup_host(host).await {
            Ok((addresses, ttl)) if !addresses.is_empty() => {
                let ttl = clamp_ttl(ttl, self.policy.min_ttl, self.policy.max_ttl);
                let stale_until = now + ttl + self.policy.stale_if_error;
                let addresses = order_addresses(addresses, &self.policy);
                let entry = CacheEntry {
                    addresses: addresses.clone(),
                    valid_until: now + ttl,
                    stale_until,
                    negative: false,
                    last_error: None,
                };
                self.cache.lock().await.insert(host.to_string(), entry);
                Ok(addresses)
            }
            Ok((_addresses, _ttl)) => {
                self.store_negative_and_fail(host, "dns lookup returned no addresses").await
            }
            Err(error) => {
                let message = error.to_string();
                let mut cache = self.cache.lock().await;
                if let Some(entry) = cache.get_mut(host)
                    && !entry.negative
                    && !entry.addresses.is_empty()
                    && now <= entry.stale_until
                {
                    entry.last_error = Some(message);
                    self.stale_answers_total.fetch_add(1, Ordering::Relaxed);
                    return Ok(order_addresses(entry.addresses.clone(), &self.policy));
                }
                drop(cache);
                self.store_negative_and_fail(host, &message).await
            }
        }
    }

    async fn store_negative_and_fail(
        &self,
        host: &str,
        message: &str,
    ) -> Result<Vec<IpAddr>, Error> {
        self.resolve_errors_total.fetch_add(1, Ordering::Relaxed);
        let now = Instant::now();
        self.cache.lock().await.insert(
            host.to_string(),
            CacheEntry {
                addresses: Vec::new(),
                valid_until: now + self.policy.negative_ttl,
                stale_until: now + self.policy.negative_ttl,
                negative: true,
                last_error: Some(message.to_string()),
            },
        );
        Err(Error::Server(format!("failed to resolve upstream hostname `{host}`: {message}")))
    }

    async fn lookup_host(&self, host: &str) -> Result<(Vec<IpAddr>, Duration), Error> {
        let lookup =
            self.resolver.lookup_ip(host).await.map_err(|error| {
                Error::Server(format!("dns lookup failed for `{host}`: {error}"))
            })?;
        let now = Instant::now();
        let ttl = lookup.valid_until().checked_duration_since(now).unwrap_or(self.policy.max_ttl);
        let mut addresses = lookup.iter().collect::<Vec<_>>();
        addresses.sort();
        addresses.dedup();
        Ok((addresses, ttl))
    }
}

fn build_resolver(policy: &UpstreamDnsPolicy) -> Result<TokioAsyncResolver, Error> {
    let mut options = ResolverOpts::default();
    options.ip_strategy = if policy.prefer_ipv4 {
        LookupIpStrategy::Ipv4thenIpv6
    } else if policy.prefer_ipv6 {
        LookupIpStrategy::Ipv6thenIpv4
    } else {
        LookupIpStrategy::Ipv4AndIpv6
    };

    if policy.resolver_addrs.is_empty() {
        return TokioAsyncResolver::tokio_from_system_conf().map_err(|error| {
            Error::Server(format!("failed to initialize system dns resolver: {error}"))
        });
    }

    let mut config = ResolverConfig::new();
    for socket_addr in &policy.resolver_addrs {
        config.add_name_server(NameServerConfig::new(*socket_addr, Protocol::Udp));
        config.add_name_server(NameServerConfig::new(*socket_addr, Protocol::Tcp));
    }
    Ok(TokioAsyncResolver::tokio(config, options))
}

fn parse_peer_addressing(peer: &UpstreamPeer) -> Result<PeerAddressing, Error> {
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
    })
}

fn build_endpoint(addressing: &PeerAddressing, ip: IpAddr) -> ResolvedUpstreamPeer {
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
    }
}

fn socket_addr_authority(socket_addr: SocketAddr) -> String {
    match socket_addr {
        SocketAddr::V4(addr) => addr.to_string(),
        SocketAddr::V6(addr) => format!("[{}]:{}", addr.ip(), addr.port()),
    }
}

fn clamp_ttl(value: Duration, min: Duration, max: Duration) -> Duration {
    value.max(min).min(max)
}

fn refresh_due(now: Instant, valid_until: Instant, refresh_before_expiry: Duration) -> bool {
    now >= valid_until || now + refresh_before_expiry >= valid_until
}

fn order_addresses(mut addresses: Vec<IpAddr>, policy: &UpstreamDnsPolicy) -> Vec<IpAddr> {
    if policy.prefer_ipv4 {
        addresses.sort_by_key(|ip| (ip.is_ipv6(), *ip));
    } else if policy.prefer_ipv6 {
        addresses.sort_by_key(|ip| (ip.is_ipv4(), *ip));
    } else {
        addresses.sort();
    }
    addresses
}

fn duration_to_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}
