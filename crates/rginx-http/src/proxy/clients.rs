use super::health::{
    ActivePeerGuard, ActiveProbeStatus, PeerFailureStatus, PeerHealthRegistry, SelectedPeers,
    UpstreamHealthSnapshot,
};
use super::*;

pub type ProxyClient = Client<HttpsConnector<HttpConnector>, HttpBody>;
pub(crate) type HealthChangeNotifier = Arc<dyn Fn(&str) + Send + Sync + 'static>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct UpstreamClientProfile {
    tls: UpstreamTls,
    protocol: UpstreamProtocol,
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
            protocol: upstream.protocol,
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

    pub fn for_upstream(&self, upstream: &Upstream) -> Result<ProxyClient, Error> {
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

fn build_client_for_profile(profile: &UpstreamClientProfile) -> Result<ProxyClient, Error> {
    let mut connector = HttpConnector::new();
    connector.enforce_http(false);
    connector.set_connect_timeout(Some(profile.connect_timeout));
    connector.set_keepalive(profile.tcp_keepalive);
    connector.set_nodelay(profile.tcp_nodelay);

    let tls_config = build_tls_config(&profile.tls)?;
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

fn build_tls_config(tls: &UpstreamTls) -> Result<ClientConfig, Error> {
    match tls {
        UpstreamTls::NativeRoots => {
            let builder = ClientConfig::builder().with_native_roots().map_err(|error| {
                Error::Server(format!("failed to load native TLS roots: {error}"))
            })?;
            Ok(builder.with_no_client_auth())
        }
        UpstreamTls::CustomCa { ca_cert_path } => {
            let roots = load_custom_ca_store(ca_cert_path)?;
            Ok(ClientConfig::builder().with_root_certificates(roots).with_no_client_auth())
        }
        UpstreamTls::Insecure => {
            let verifier = Arc::new(InsecureServerCertVerifier::new());
            Ok(ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(verifier)
                .with_no_client_auth())
        }
    }
}

pub(super) fn load_custom_ca_store(path: &Path) -> Result<RootCertStore, Error> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let certs =
        rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>().map_err(|error| {
            Error::Server(format!(
                "failed to parse custom CA certificates from `{}`: {error}",
                path.display()
            ))
        })?;

    let mut roots = RootCertStore::empty();
    if certs.is_empty() {
        let der = std::fs::read(path)?;
        roots.add(CertificateDer::from(der)).map_err(|error| {
            Error::Server(format!(
                "failed to add DER custom CA certificate `{}`: {error}",
                path.display()
            ))
        })?;
        return Ok(roots);
    }

    let (added, _ignored) = roots.add_parsable_certificates(certs);
    if added == 0 || roots.is_empty() {
        return Err(Error::Server(format!(
            "no valid CA certificates were loaded from `{}`",
            path.display()
        )));
    }

    Ok(roots)
}

#[derive(Debug)]
struct InsecureServerCertVerifier {
    supported_schemes: Vec<SignatureScheme>,
}

impl InsecureServerCertVerifier {
    fn new() -> Self {
        let supported_schemes = rustls::crypto::aws_lc_rs::default_provider()
            .signature_verification_algorithms
            .supported_schemes();
        Self { supported_schemes }
    }
}

impl ServerCertVerifier for InsecureServerCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.supported_schemes.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    use rginx_core::{
        ActiveHealthCheck, ConfigSnapshot, Listener, RuntimeSettings, Server, Upstream,
        UpstreamLoadBalance, UpstreamPeer, UpstreamProtocol, UpstreamSettings, UpstreamTls,
        VirtualHost,
    };

    use super::ProxyClients;

    #[test]
    fn peer_health_snapshot_delegates_to_registry() {
        let upstream = Arc::new(Upstream::new(
            "backend".to_string(),
            vec![UpstreamPeer {
                url: "http://127.0.0.1:9000".to_string(),
                scheme: "http".to_string(),
                authority: "127.0.0.1:9000".to_string(),
                weight: 1,
                backup: false,
            }],
            UpstreamTls::NativeRoots,
            UpstreamSettings {
                protocol: UpstreamProtocol::Auto,
                load_balance: UpstreamLoadBalance::RoundRobin,
                server_name_override: None,
                request_timeout: Duration::from_secs(30),
                connect_timeout: Duration::from_secs(30),
                write_timeout: Duration::from_secs(30),
                idle_timeout: Duration::from_secs(30),
                pool_idle_timeout: Some(Duration::from_secs(90)),
                pool_max_idle_per_host: usize::MAX,
                tcp_keepalive: None,
                tcp_nodelay: false,
                http2_keep_alive_interval: None,
                http2_keep_alive_timeout: Duration::from_secs(20),
                http2_keep_alive_while_idle: false,
                max_replayable_request_body_bytes: 64 * 1024,
                unhealthy_after_failures: 2,
                unhealthy_cooldown: Duration::from_secs(10),
                active_health_check: Some(ActiveHealthCheck {
                    path: "/healthz".to_string(),
                    grpc_service: None,
                    interval: Duration::from_secs(5),
                    timeout: Duration::from_secs(2),
                    healthy_successes_required: 2,
                }),
            },
        ));
        let server = Server {
            listen_addr: "127.0.0.1:8080".parse().unwrap(),
            trusted_proxies: Vec::new(),
            keep_alive: true,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout: None,
            request_body_read_timeout: None,
            response_write_timeout: None,
            access_log_format: None,
            tls: None,
        };
        let snapshot = ConfigSnapshot {
            runtime: RuntimeSettings {
                shutdown_timeout: Duration::from_secs(1),
                worker_threads: None,
                accept_workers: 1,
            },
            server: server.clone(),
            listeners: vec![Listener {
                id: "default".to_string(),
                name: "default".to_string(),
                server,
                tls_termination_enabled: false,
                proxy_protocol_enabled: false,
            }],
            default_vhost: VirtualHost {
                id: "server".to_string(),
                server_names: Vec::new(),
                routes: Vec::new(),
                tls: None,
            },
            vhosts: Vec::new(),
            upstreams: HashMap::from([("backend".to_string(), upstream)]),
        };

        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");
        let snapshot = clients.peer_health_snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].upstream_name, "backend");
        assert_eq!(snapshot[0].peers.len(), 1);
        assert_eq!(snapshot[0].peers[0].peer_url, "http://127.0.0.1:9000");
    }
}
