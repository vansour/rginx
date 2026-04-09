use super::health::{
    ActivePeerGuard, ActiveProbeStatus, PeerFailureStatus, PeerHealthRegistry, SelectedPeers,
    UpstreamHealthSnapshot,
};
use super::*;
use rginx_core::{ClientIdentity, TlsVersion};
use rustls::client::WebPkiServerVerifier;
use rustls_native_certs::load_native_certs;
use std::path::{Path, PathBuf};

pub type ProxyClient = Client<HttpsConnector<HttpConnector>, HttpBody>;
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

    let tls_config = build_tls_config(
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

fn build_tls_config(
    tls: &UpstreamTls,
    versions: Option<&[TlsVersion]>,
    server_verify_depth: Option<u32>,
    server_crl_path: Option<&Path>,
    client_identity: Option<&ClientIdentity>,
    server_name: bool,
) -> Result<ClientConfig, Error> {
    let builder = build_client_config_builder(versions);
    let mut config = match tls {
        UpstreamTls::NativeRoots => {
            let roots = load_native_root_store()?;
            let verifier = build_server_cert_verifier(roots, server_verify_depth, server_crl_path)?;
            build_client_config_with_identity(
                builder.dangerous().with_custom_certificate_verifier(verifier),
                client_identity,
            )
        }
        UpstreamTls::CustomCa { ca_cert_path } => {
            let roots = load_custom_ca_store(ca_cert_path)?;
            let verifier = build_server_cert_verifier(roots, server_verify_depth, server_crl_path)?;
            build_client_config_with_identity(
                builder.dangerous().with_custom_certificate_verifier(verifier),
                client_identity,
            )
        }
        UpstreamTls::Insecure => {
            let verifier = Arc::new(InsecureServerCertVerifier::new());
            build_client_config_with_identity(
                builder.dangerous().with_custom_certificate_verifier(verifier),
                client_identity,
            )
        }
    }?;
    config.enable_sni = server_name;
    Ok(config)
}

fn build_server_cert_verifier(
    roots: RootCertStore,
    server_verify_depth: Option<u32>,
    server_crl_path: Option<&Path>,
) -> Result<Arc<dyn ServerCertVerifier>, Error> {
    let builder = if let Some(crl_path) = server_crl_path {
        WebPkiServerVerifier::builder(roots.into())
            .with_crls(load_certificate_revocation_lists(crl_path)?)
    } else {
        WebPkiServerVerifier::builder(roots.into())
    };
    let verifier = builder.build().map_err(|error| {
        Error::Server(format!("failed to build upstream certificate verifier: {error}"))
    })?;
    Ok(Arc::new(DepthLimitedServerCertVerifier::new(verifier, server_verify_depth)))
}

fn build_client_config_builder(
    versions: Option<&[TlsVersion]>,
) -> rustls::ConfigBuilder<ClientConfig, rustls::WantsVerifier> {
    match versions {
        Some(versions) => ClientConfig::builder_with_protocol_versions(&rustls_versions(versions)),
        None => ClientConfig::builder(),
    }
}

fn build_client_config_with_identity(
    builder: rustls::ConfigBuilder<ClientConfig, rustls::client::WantsClientCert>,
    client_identity: Option<&ClientIdentity>,
) -> Result<ClientConfig, Error> {
    match client_identity {
        Some(client_identity) => {
            let cert_chain = load_certificate_chain(&client_identity.cert_path)?;
            let key_der = load_private_key(&client_identity.key_path)?;
            builder.with_client_auth_cert(cert_chain, key_der).map_err(|error| {
                Error::Server(format!(
                    "failed to configure upstream mTLS identity from `{}` and `{}`: {error}",
                    client_identity.cert_path.display(),
                    client_identity.key_path.display()
                ))
            })
        }
        None => Ok(builder.with_no_client_auth()),
    }
}

pub(super) fn load_custom_ca_store(path: &Path) -> Result<RootCertStore, Error> {
    let certs = load_certificate_chain(path)?;
    let mut roots = RootCertStore::empty();
    let (added, _ignored) = roots.add_parsable_certificates(certs);
    if added == 0 || roots.is_empty() {
        return Err(Error::Server(format!(
            "no valid CA certificates were loaded from `{}`",
            path.display()
        )));
    }

    Ok(roots)
}

fn load_native_root_store() -> Result<RootCertStore, Error> {
    let result = load_native_certs();
    if result.certs.is_empty() && !result.errors.is_empty() {
        return Err(Error::Server(format!("failed to load native TLS roots: {:?}", result.errors)));
    }

    let mut roots = RootCertStore::empty();
    let (added, _ignored) = roots.add_parsable_certificates(result.certs);
    if added == 0 || roots.is_empty() {
        return Err(Error::Server("no valid native TLS roots were loaded".to_string()));
    }
    Ok(roots)
}

fn load_certificate_revocation_lists(
    path: &Path,
) -> Result<Vec<rustls::pki_types::CertificateRevocationListDer<'static>>, Error> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let crls =
        rustls_pemfile::crls(&mut reader).collect::<Result<Vec<_>, _>>().map_err(|error| {
            Error::Server(format!("failed to parse CRLs from `{}`: {error}", path.display()))
        })?;

    if !crls.is_empty() {
        return Ok(crls);
    }

    Ok(vec![rustls::pki_types::CertificateRevocationListDer::from(std::fs::read(path)?)])
}

fn load_certificate_chain(path: &Path) -> Result<Vec<CertificateDer<'static>>, Error> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let certs =
        rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>().map_err(|error| {
            Error::Server(format!(
                "failed to parse certificates from `{}`: {error}",
                path.display()
            ))
        })?;

    if certs.is_empty() {
        let der = std::fs::read(path)?;
        return Ok(vec![CertificateDer::from(der)]);
    }

    Ok(certs)
}

fn load_private_key(path: &Path) -> Result<rustls::pki_types::PrivateKeyDer<'static>, Error> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    rustls_pemfile::private_key(&mut reader)
        .map_err(|error| {
            Error::Server(format!("failed to parse private key `{}`: {error}", path.display()))
        })?
        .ok_or_else(|| {
            Error::Server(format!(
                "private key file `{}` did not contain a supported PEM private key",
                path.display()
            ))
        })
}

fn rustls_versions(versions: &[TlsVersion]) -> Vec<&'static rustls::SupportedProtocolVersion> {
    versions
        .iter()
        .map(|version| match version {
            TlsVersion::Tls12 => &rustls::version::TLS12,
            TlsVersion::Tls13 => &rustls::version::TLS13,
        })
        .collect()
}

#[derive(Debug)]
struct InsecureServerCertVerifier {
    supported_schemes: Vec<SignatureScheme>,
}

#[derive(Debug)]
struct DepthLimitedServerCertVerifier {
    inner: Arc<dyn ServerCertVerifier>,
    verify_depth: Option<u32>,
}

impl DepthLimitedServerCertVerifier {
    fn new(inner: Arc<dyn ServerCertVerifier>, verify_depth: Option<u32>) -> Self {
        Self { inner, verify_depth }
    }
}

impl ServerCertVerifier for DepthLimitedServerCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if let Some(max_depth) = self.verify_depth {
            let presented_chain_depth = 1usize.saturating_add(intermediates.len());
            if presented_chain_depth > max_depth as usize {
                return Err(rustls::Error::General(format!(
                    "upstream certificate chain exceeds configured verify_depth `{max_depth}`: got {presented_chain_depth} certificate(s)"
                )));
            }
        }

        self.inner.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
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
                server_name: true,
                server_name_override: None,
                tls_versions: None,
                server_verify_depth: None,
                server_crl_path: None,
                client_identity: None,
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
            default_certificate: None,
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
