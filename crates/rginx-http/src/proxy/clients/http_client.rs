use std::future::{Ready, ready};
use std::io;
use std::net::SocketAddr;
use std::task::{Context, Poll};

use rustls::ClientConfig;
use tokio::sync::Mutex;
use tower_service::Service;

use super::*;

const ENDPOINT_CLIENT_CACHE_MIN_CAPACITY: usize = 16;
const ENDPOINT_CLIENT_CACHE_MAX_CAPACITY: usize = 1024;
const ENDPOINT_CLIENT_CACHE_POOL_MULTIPLIER: usize = 4;

#[derive(Clone)]
pub(crate) struct HttpProxyClient {
    // Hyper pools by selected socket. `pool_max_idle_per_host` applies to every
    // endpoint client, so effective idle capacity is per live endpoint until LRU
    // eviction trims stale DNS endpoints from this bounded cache.
    pub(super) endpoint_clients: Arc<Mutex<EndpointClientCache>>,
    pub(super) resolver: Arc<UpstreamResolver>,
    pub(super) profile: UpstreamClientProfile,
    pub(super) tls_config: ClientConfig,
    pub(super) server_name_override: Option<ServerName<'static>>,
}

pub(super) struct EndpointClientCache {
    pub(super) entries: HashMap<SocketAddr, EndpointClientCacheEntry>,
    capacity: usize,
    next_access: u64,
}

pub(super) struct EndpointClientCacheEntry {
    client: HyperProxyClient,
    last_used: u64,
}

#[derive(Clone, Debug)]
pub(super) struct FixedEndpointResolver {
    socket_addr: SocketAddr,
}

impl HttpProxyClient {
    pub(super) async fn client_for_peer(
        &self,
        peer: &ResolvedUpstreamPeer,
    ) -> Result<HyperProxyClient, Error> {
        let mut endpoint_clients = self.endpoint_clients.lock().await;
        if let Some(client) = endpoint_clients.get(peer.socket_addr) {
            return Ok(client);
        }

        // Keep construction under the cache lock so concurrent requests for the
        // same newly resolved endpoint do not build duplicate Hyper clients.
        let client = build_hyper_client_for_endpoint(self, peer.socket_addr)?;
        Ok(endpoint_clients.insert(peer.socket_addr, client))
    }
}

impl EndpointClientCache {
    pub(super) fn new(capacity: usize) -> Self {
        Self { entries: HashMap::new(), capacity: capacity.max(1), next_access: 0 }
    }

    pub(super) fn get(&mut self, socket_addr: SocketAddr) -> Option<HyperProxyClient> {
        let last_used = self.next_access();
        self.entries.get_mut(&socket_addr).map(|entry| {
            entry.last_used = last_used;
            entry.client.clone()
        })
    }

    pub(super) fn insert(
        &mut self,
        socket_addr: SocketAddr,
        client: HyperProxyClient,
    ) -> HyperProxyClient {
        if !self.entries.contains_key(&socket_addr) && self.entries.len() >= self.capacity {
            self.evict_lru();
        }
        let last_used = self.next_access();
        self.entries
            .insert(socket_addr, EndpointClientCacheEntry { client: client.clone(), last_used });
        client
    }

    fn evict_lru(&mut self) {
        let Some(socket_addr) = self
            .entries
            .iter()
            .min_by_key(|(_socket_addr, entry)| entry.last_used)
            .map(|(socket_addr, _entry)| *socket_addr)
        else {
            return;
        };
        self.entries.remove(&socket_addr);
    }

    fn next_access(&mut self) -> u64 {
        self.next_access = self.next_access.saturating_add(1);
        self.next_access
    }
}

impl FixedEndpointResolver {
    fn new(socket_addr: SocketAddr) -> Self {
        Self { socket_addr }
    }
}

impl Service<hyper_util::client::legacy::connect::dns::Name> for FixedEndpointResolver {
    type Response = std::vec::IntoIter<SocketAddr>;
    type Error = io::Error;
    type Future = Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _name: hyper_util::client::legacy::connect::dns::Name) -> Self::Future {
        ready(Ok(vec![self.socket_addr].into_iter()))
    }
}

pub(super) fn build_hyper_client_for_endpoint(
    client: &HttpProxyClient,
    socket_addr: SocketAddr,
) -> Result<HyperProxyClient, Error> {
    let profile = &client.profile;
    let mut connector = HttpConnector::new_with_resolver(FixedEndpointResolver::new(socket_addr));
    connector.enforce_http(false);
    connector.set_connect_timeout(Some(profile.connect_timeout));
    connector.set_keepalive(profile.tcp_keepalive);
    connector.set_nodelay(profile.tcp_nodelay);

    let builder =
        HttpsConnectorBuilder::new().with_tls_config(client.tls_config.clone()).https_or_http();
    let builder = if let Some(server_name_override) = &client.server_name_override {
        builder
            .with_server_name_resolver(FixedServerNameResolver::new(server_name_override.clone()))
    } else {
        builder
    };
    let connector = match profile.protocol {
        UpstreamProtocol::Auto => builder.enable_all_versions().wrap_connector(connector),
        UpstreamProtocol::Http1 => builder.enable_http1().wrap_connector(connector),
        UpstreamProtocol::Http2 => builder.enable_http2().wrap_connector(connector),
        UpstreamProtocol::Http3 => unreachable!("handled before hyper connector construction"),
    };

    let mut client_builder = Client::builder(TokioExecutor::new());
    client_builder.timer(TokioTimer::new());
    client_builder.pool_timer(TokioTimer::new());
    client_builder.set_host(false);
    client_builder.pool_idle_timeout(profile.pool_idle_timeout);
    client_builder.pool_max_idle_per_host(profile.pool_max_idle_per_host);
    if let Some(interval) = profile.http2_keep_alive_interval {
        client_builder.http2_keep_alive_interval(interval);
        client_builder.http2_keep_alive_timeout(profile.http2_keep_alive_timeout);
        client_builder.http2_keep_alive_while_idle(profile.http2_keep_alive_while_idle);
    }
    if profile.protocol == UpstreamProtocol::Http2 {
        client_builder.http2_only(true);
    }

    Ok(client_builder.build(connector))
}

pub(super) fn endpoint_client_cache_capacity(pool_max_idle_per_host: usize) -> usize {
    pool_max_idle_per_host
        .saturating_mul(ENDPOINT_CLIENT_CACHE_POOL_MULTIPLIER)
        .clamp(ENDPOINT_CLIENT_CACHE_MIN_CAPACITY, ENDPOINT_CLIENT_CACHE_MAX_CAPACITY)
}
