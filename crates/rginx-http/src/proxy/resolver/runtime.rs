use std::net::IpAddr;
use std::str::FromStr;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use rginx_core::{Error, UpstreamPeer};

use super::endpoint::{
    build_endpoint, build_resolver, clamp_ttl, duration_to_ms, order_addresses,
    parse_peer_addressing, refresh_due,
};
use super::*;

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
