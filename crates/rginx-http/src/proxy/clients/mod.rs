use std::error::Error as StdError;
use std::path::PathBuf;

use super::health::{
    ActivePeerGuard, ActiveProbeStatus, PeerFailureStatus, PeerHealthRegistry, SelectedPeers,
    UpstreamHealthSnapshot,
};
use super::*;
use rginx_core::{ClientIdentity, TlsVersion};

mod http3;
#[cfg(test)]
mod tests;
mod tls;

#[cfg(test)]
pub(super) use tls::load_custom_ca_store;

pub type HyperProxyClient = Client<HttpsConnector<HttpConnector>, HttpBody>;
pub(crate) type HealthChangeNotifier = Arc<dyn Fn(&str) + Send + Sync + 'static>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct UpstreamClientProfile {
    tls: UpstreamTls,
    tls_versions: Option<Vec<TlsVersion>>,
    server_verify_depth: Option<u32>,
    server_crl_path: Option<PathBuf>,
    client_identity: Option<ClientIdentity>,
    protocol: UpstreamProtocol,
    server_name: bool,
    server_name_override: Option<String>,
    connect_timeout: Duration,
    pool_idle_timeout: Option<Duration>,
    pool_max_idle_per_host: usize,
    tcp_keepalive: Option<Duration>,
    tcp_nodelay: bool,
    http2_keep_alive_interval: Option<Duration>,
    http2_keep_alive_timeout: Duration,
    http2_keep_alive_while_idle: bool,
}

impl UpstreamClientProfile {
    fn from_upstream(upstream: &Upstream) -> Self {
        Self {
            tls: upstream.tls.clone(),
            tls_versions: upstream.tls_versions.clone(),
            server_verify_depth: upstream.server_verify_depth,
            server_crl_path: upstream.server_crl_path.clone(),
            client_identity: upstream.client_identity.clone(),
            protocol: upstream.protocol,
            server_name: upstream.server_name,
            server_name_override: upstream.server_name_override.clone(),
            connect_timeout: upstream.connect_timeout,
            pool_idle_timeout: upstream.pool_idle_timeout,
            pool_max_idle_per_host: upstream.pool_max_idle_per_host,
            tcp_keepalive: upstream.tcp_keepalive,
            tcp_nodelay: upstream.tcp_nodelay,
            http2_keep_alive_interval: upstream.http2_keep_alive_interval,
            http2_keep_alive_timeout: upstream.http2_keep_alive_timeout,
            http2_keep_alive_while_idle: upstream.http2_keep_alive_while_idle,
        }
    }
}

#[derive(Clone)]
pub struct ProxyClients {
    clients: Arc<HashMap<UpstreamClientProfile, ProxyClient>>,
    health: PeerHealthRegistry,
}

#[derive(Clone)]
pub(crate) enum ProxyClient {
    Http(Box<HyperProxyClient>),
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

        Ok(Self { clients: Arc::new(clients), health })
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

    pub(super) fn select_peers(
        &self,
        upstream: &Upstream,
        client_ip: std::net::IpAddr,
        limit: usize,
    ) -> SelectedPeers {
        self.health.select_peers(upstream, client_ip, limit)
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

    pub(crate) fn peer_health_snapshot(&self) -> Vec<UpstreamHealthSnapshot> {
        self.health.snapshot()
    }

    #[cfg(test)]
    pub(super) fn cached_client_count(&self) -> usize {
        self.clients.len()
    }
}

impl ProxyClient {
    pub async fn request(
        &self,
        upstream: &Upstream,
        peer: &UpstreamPeer,
        request: Request<HttpBody>,
    ) -> Result<Response<HttpBody>, Error> {
        match self {
            Self::Http(client) => client
                .request(request)
                .await
                .map(|response| response.map(crate::handler::boxed_body))
                .map_err(|error| {
                    Error::Server(format_error_chain("upstream request failed", &error))
                }),
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

fn build_client_for_profile(profile: &UpstreamClientProfile) -> Result<ProxyClient, Error> {
    if profile.protocol == UpstreamProtocol::Http3 {
        let client_config = tls::build_http3_client_config(
            &profile.tls,
            profile.tls_versions.as_deref(),
            profile.server_verify_depth,
            profile.server_crl_path.as_deref(),
            profile.client_identity.as_ref(),
            profile.server_name,
        )?;
        return Ok(ProxyClient::Http3(http3::Http3Client::new(
            client_config,
            profile.connect_timeout,
        )));
    }

    let mut connector = HttpConnector::new();
    connector.enforce_http(false);
    connector.set_connect_timeout(Some(profile.connect_timeout));
    connector.set_keepalive(profile.tcp_keepalive);
    connector.set_nodelay(profile.tcp_nodelay);

    let tls_config = tls::build_tls_config(
        &profile.tls,
        profile.tls_versions.as_deref(),
        profile.server_verify_depth,
        profile.server_crl_path.as_deref(),
        profile.client_identity.as_ref(),
        profile.server_name,
    )?;
    let builder = HttpsConnectorBuilder::new().with_tls_config(tls_config).https_or_http();
    let builder = if let Some(server_name_override) = &profile.server_name_override {
        let server_name = ServerName::try_from(server_name_override.clone()).map_err(|error| {
            Error::Server(format!(
                "invalid TLS server_name_override `{server_name_override}`: {error}"
            ))
        })?;
        builder.with_server_name_resolver(FixedServerNameResolver::new(server_name))
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

    Ok(ProxyClient::Http(Box::new(client_builder.build(connector))))
}
