//! Cached upstream proxy client selection for HTTP/1.1, HTTP/2, and HTTP/3.

use super::health::{
    ActivePeerGuard, ActiveProbeStatus, PeerFailureStatus, PeerHealthRegistry, SelectedPeers,
    UpstreamHealthSnapshot,
};
use super::*;
use std::error::Error as StdError;

mod factory;
mod http3;
mod http_client;
mod profile;
#[cfg(test)]
mod tests;
mod tls;

#[cfg(test)]
pub(super) use tls::load_custom_ca_store;

type HyperProxyClient = Client<HttpsConnector<HttpConnector<FixedEndpointResolver>>, HttpBody>;
pub(crate) type HealthChangeNotifier = Arc<dyn Fn(&str) + Send + Sync + 'static>;

use factory::build_client_for_profile;
pub(crate) use http_client::HttpProxyClient;
#[cfg(test)]
use http_client::build_hyper_client_for_endpoint;
use http_client::{EndpointClientCache, FixedEndpointResolver, endpoint_client_cache_capacity};
use profile::UpstreamClientProfile;

#[derive(Clone)]
pub struct ProxyClients {
    upstreams: Arc<HashMap<String, Arc<Upstream>>>,
    clients: Arc<HashMap<UpstreamClientProfile, ProxyClient>>,
    health: PeerHealthRegistry,
}

#[derive(Clone)]
pub(crate) enum ProxyClient {
    Http(Arc<HttpProxyClient>),
    Http3(http3::Http3Client),
}

impl ProxyClients {
    pub fn from_config(config: &ConfigSnapshot) -> Result<Self, Error> {
        Self::from_config_with_health_notifier(config, None)
    }

    pub(crate) fn from_config_with_health_notifier(
        config: &ConfigSnapshot,
        notifier: Option<HealthChangeNotifier>,
    ) -> Result<Self, Error> {
        let profiles = config
            .upstreams
            .values()
            .map(|upstream| UpstreamClientProfile::from_upstream(upstream.as_ref()))
            .collect::<HashSet<_>>();

        let mut clients = HashMap::new();
        for profile in profiles {
            let client = build_client_for_profile(&profile)?;
            clients.insert(profile, client);
        }

        let health = if let Some(notifier) = notifier {
            PeerHealthRegistry::from_config_with_notifier(config, Some(notifier))
        } else {
            PeerHealthRegistry::from_config(config)
        };

        Ok(Self {
            upstreams: Arc::new(config.upstreams.clone()),
            clients: Arc::new(clients),
            health,
        })
    }

    pub(crate) fn for_upstream(&self, upstream: &Upstream) -> Result<ProxyClient, Error> {
        let profile = UpstreamClientProfile::from_upstream(upstream);
        self.clients.get(&profile).cloned().ok_or_else(|| {
            Error::Server(format!(
                "missing cached proxy client for upstream `{}` with TLS profile {:?}",
                upstream.name, profile
            ))
        })
    }

    pub(super) async fn select_peers(
        &self,
        upstream: &Upstream,
        client_ip: std::net::IpAddr,
        limit: usize,
    ) -> SelectedPeers {
        match self.for_upstream(upstream) {
            Ok(client) => self.health.select_peers(&client, upstream, client_ip, limit).await,
            Err(_) => SelectedPeers { peers: Vec::new(), skipped_unhealthy: 0 },
        }
    }

    pub(super) fn record_peer_success(&self, upstream_name: &str, peer_url: &str) -> bool {
        self.health.record_success(upstream_name, peer_url)
    }

    pub(super) fn record_peer_failure(
        &self,
        upstream_name: &str,
        peer_url: &str,
    ) -> PeerFailureStatus {
        self.health.record_failure(upstream_name, peer_url)
    }

    pub(super) fn record_active_peer_success(
        &self,
        upstream_name: &str,
        peer_url: &str,
        healthy_successes_required: u32,
    ) -> ActiveProbeStatus {
        self.health.record_active_success(upstream_name, peer_url, healthy_successes_required)
    }

    pub(crate) fn record_active_peer_failure(&self, upstream_name: &str, peer_url: &str) -> bool {
        self.health.record_active_failure(upstream_name, peer_url)
    }

    pub(super) fn track_active_request(
        &self,
        upstream_name: &str,
        peer_url: &str,
    ) -> ActivePeerGuard {
        self.health.track_active_request(upstream_name, peer_url)
    }

    pub(crate) async fn peer_health_snapshot(&self) -> Vec<UpstreamHealthSnapshot> {
        let mut upstreams = self.upstreams.values().cloned().collect::<Vec<_>>();
        upstreams.sort_by(|left, right| left.name.cmp(&right.name));

        futures_util::future::join_all(upstreams.into_iter().map(|upstream| async move {
            let client = self.for_upstream(upstream.as_ref()).ok()?;
            let resolver = client.resolver_snapshot().await;
            let mut endpoints = Vec::new();
            for peer in &upstream.peers {
                endpoints.extend(client.cached_peer_endpoints(peer).await.ok()?);
            }
            endpoints.sort_by(|left, right| left.endpoint_key.cmp(&right.endpoint_key));
            endpoints.dedup_by(|left, right| left.endpoint_key == right.endpoint_key);

            Some(self.health.snapshot_for_upstream(upstream.as_ref(), resolver, endpoints))
        }))
        .await
        .into_iter()
        .flatten()
        .collect()
    }

    #[cfg(test)]
    pub(super) fn cached_client_count(&self) -> usize {
        self.clients.len()
    }
}

impl ProxyClient {
    pub(crate) async fn resolve_peer(
        &self,
        peer: &UpstreamPeer,
    ) -> Result<Vec<ResolvedUpstreamPeer>, Error> {
        match self {
            Self::Http(client) => client.resolver.resolve_peer(peer).await,
            Self::Http3(client) => client.resolve_peer(peer).await,
        }
    }

    pub(crate) async fn cached_peer_endpoints(
        &self,
        peer: &UpstreamPeer,
    ) -> Result<Vec<ResolvedUpstreamPeer>, Error> {
        match self {
            Self::Http(client) => client.resolver.cached_peer_endpoints(peer).await,
            Self::Http3(client) => client.cached_peer_endpoints(peer).await,
        }
    }

    pub(crate) async fn resolver_snapshot(&self) -> UpstreamResolverRuntimeSnapshot {
        match self {
            Self::Http(client) => client.resolver.snapshot().await,
            Self::Http3(client) => client.resolver_snapshot().await,
        }
    }

    pub async fn request(
        &self,
        upstream: &Upstream,
        peer: &ResolvedUpstreamPeer,
        request: Request<HttpBody>,
    ) -> Result<Response<HttpBody>, Error> {
        match self {
            Self::Http(client) => {
                let client = client.client_for_peer(peer)?;
                client
                    .request(request)
                    .await
                    .map(|response| response.map(crate::handler::boxed_body))
                    .map_err(|error| {
                        Error::Server(format_error_chain("upstream request failed", &error))
                    })
            }
            Self::Http3(client) => client.request(upstream, peer, request).await,
        }
    }
}

fn format_error_chain(prefix: &str, error: &(dyn StdError + 'static)) -> String {
    let mut message = format!("{prefix}: {error}");
    let mut current = error.source();
    while let Some(source) = current {
        message.push_str(": ");
        message.push_str(&source.to_string());
        current = source.source();
    }
    message
}
