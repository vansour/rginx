use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock as StdRwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use http::StatusCode;
use rginx_core::{ConfigSnapshot, Listener, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Notify, RwLock, watch};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use x509_parser::extensions::{GeneralName, ParsedExtension};
use x509_parser::prelude::{FromDer, X509Certificate};

use crate::proxy::{HealthChangeNotifier, ProxyClients, UpstreamHealthSnapshot};
use crate::rate_limit::RateLimiters;
use crate::tls::build_tls_acceptor;
use crate::tls::ocsp::ocsp_responder_urls_for_certificate;

mod connections;
mod lifecycle;
mod snapshot_bus;
mod tls_runtime;
mod traffic;
mod upstreams;

const RECENT_WINDOW_SECS: u64 = 60;
const MAX_RECENT_WINDOW_SECS: u64 = 300;
const TLS_EXPIRY_WARNING_DAYS: i64 = 30;

struct PreparedState {
    config: Arc<ConfigSnapshot>,
    clients: ProxyClients,
    listener_tls_acceptors: HashMap<String, Option<TlsAcceptor>>,
    retired_listeners: Vec<Listener>,
}

include!("state/snapshots.rs");
include!("state/counters.rs");
include!("state/helpers.rs");

#[cfg(test)]
pub(crate) use crate::validate_config_transition;
pub use connections::ActiveConnectionGuard;
#[cfg(test)]
pub(crate) use tls_runtime::inspect_certificate;
pub use tls_runtime::{
    tls_reloadable_fields, tls_restart_required_fields, tls_runtime_snapshot_for_config,
};

#[derive(Clone)]
pub struct SharedState {
    inner: Arc<RwLock<ActiveState>>,
    revisions: watch::Sender<u64>,
    rate_limiters: RateLimiters,
    snapshot_version: Arc<AtomicU64>,
    snapshot_notify: Arc<Notify>,
    snapshot_components: Arc<SnapshotComponentVersions>,
    listener_tls_acceptors: Arc<RwLock<HashMap<String, Option<TlsAcceptor>>>>,
    listener_active_connections: Arc<StdRwLock<HashMap<String, Arc<AtomicUsize>>>>,
    retired_listeners: Arc<StdRwLock<HashMap<String, Listener>>>,
    background_tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
    active_connections: Arc<AtomicUsize>,
    counters: Arc<HttpCounters>,
    traffic_stats: Arc<StdRwLock<TrafficStatsIndex>>,
    traffic_component_versions: Arc<StdRwLock<TrafficComponentVersions>>,
    upstream_stats: Arc<StdRwLock<HashMap<String, UpstreamStatsEntry>>>,
    upstream_component_versions: Arc<StdRwLock<HashMap<String, u64>>>,
    peer_health_component_versions: Arc<StdRwLock<HashMap<String, u64>>>,
    reload_history: Arc<Mutex<ReloadHistory>>,
    ocsp_statuses: Arc<StdRwLock<HashMap<String, OcspRuntimeStatusEntry>>>,
    request_ids: Arc<AtomicU64>,
    config_path: Option<Arc<PathBuf>>,
}

#[derive(Debug, Clone, Default)]
struct OcspRuntimeStatusEntry {
    last_refresh_unix_ms: Option<u64>,
    refreshes_total: u64,
    failures_total: u64,
    last_error: Option<String>,
}

impl SharedState {
    pub fn from_config(config: ConfigSnapshot) -> Result<Self> {
        Self::from_parts(config, None)
    }

    pub fn from_config_path(config_path: PathBuf, config: ConfigSnapshot) -> Result<Self> {
        Self::from_parts(config, Some(config_path))
    }

    fn from_parts(config: ConfigSnapshot, config_path: Option<PathBuf>) -> Result<Self> {
        let snapshot_version = Arc::new(AtomicU64::new(0));
        let snapshot_notify = Arc::new(Notify::new());
        let snapshot_components = Arc::new(SnapshotComponentVersions::default());
        let peer_health_component_versions = Arc::new(StdRwLock::new(HashMap::new()));
        let prepared = prepare_state(
            config,
            Some(build_peer_health_notifier(
                snapshot_version.clone(),
                snapshot_notify.clone(),
                snapshot_components.clone(),
                peer_health_component_versions.clone(),
            )),
        )?;
        let revision = 0u64;
        let (revisions, _rx) = watch::channel(revision);
        let rate_limiters = RateLimiters::default();
        let listener_active_connections = prepared
            .config
            .listeners
            .iter()
            .map(|listener| (listener.id.clone(), Arc::new(AtomicUsize::new(0))))
            .collect::<HashMap<_, _>>();
        let traffic_stats =
            Arc::new(StdRwLock::new(build_traffic_stats_index(prepared.config.as_ref(), None)));
        let traffic_component_versions = Arc::new(StdRwLock::new(
            build_traffic_component_versions(prepared.config.as_ref(), None),
        ));
        let upstream_stats =
            Arc::new(StdRwLock::new(build_upstream_stats_map(prepared.config.as_ref(), None)));
        let upstream_component_versions =
            Arc::new(StdRwLock::new(build_upstream_name_versions(prepared.config.as_ref(), None)));
        *peer_health_component_versions.write().unwrap_or_else(|poisoned| poisoned.into_inner()) =
            build_upstream_name_versions(prepared.config.as_ref(), None);
        let ocsp_statuses = Arc::new(StdRwLock::new(HashMap::new()));

        Ok(Self {
            inner: Arc::new(RwLock::new(ActiveState {
                revision,
                config: prepared.config,
                clients: prepared.clients,
            })),
            revisions,
            rate_limiters,
            snapshot_version,
            snapshot_notify,
            snapshot_components,
            listener_tls_acceptors: Arc::new(RwLock::new(prepared.listener_tls_acceptors)),
            listener_active_connections: Arc::new(StdRwLock::new(listener_active_connections)),
            retired_listeners: Arc::new(StdRwLock::new(HashMap::new())),
            background_tasks: Arc::new(Mutex::new(Vec::new())),
            active_connections: Arc::new(AtomicUsize::new(0)),
            counters: Arc::new(HttpCounters::default()),
            traffic_stats,
            traffic_component_versions,
            upstream_stats,
            upstream_component_versions,
            peer_health_component_versions,
            reload_history: Arc::new(Mutex::new(ReloadHistory::default())),
            ocsp_statuses,
            request_ids: Arc::new(AtomicU64::new(1)),
            config_path: config_path.map(Arc::new),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use http::StatusCode;
    use rcgen::{
        BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    };
    use rginx_core::{
        Listener, ReturnAction, Route, RouteAccessControl, RouteAction, RouteMatcher,
        RuntimeSettings, Server, Upstream, UpstreamLoadBalance, UpstreamPeer, UpstreamProtocol,
        UpstreamSettings, UpstreamTls, VirtualHost,
    };

    use super::{
        ConfigSnapshot, ReloadOutcomeSnapshot, SharedState, TlsHandshakeFailureReason,
        inspect_certificate, validate_config_transition,
    };

    fn snapshot(listen: &str) -> ConfigSnapshot {
        let server = Server {
            listen_addr: listen.parse().unwrap(),
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
        ConfigSnapshot {
            runtime: RuntimeSettings {
                shutdown_timeout: Duration::from_secs(10),
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
            upstreams: HashMap::new(),
        }
    }

    fn snapshot_with_upstream(listen: &str) -> ConfigSnapshot {
        let mut snapshot = snapshot(listen);
        snapshot.upstreams.insert(
            "backend".to_string(),
            Arc::new(Upstream::new(
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
                    active_health_check: None,
                },
            )),
        );
        snapshot
    }

    fn snapshot_with_routes(listen: &str) -> ConfigSnapshot {
        let mut snapshot = snapshot(listen);
        snapshot.default_vhost.routes = vec![Route {
            id: "server/routes[0]|exact:/".to_string(),
            matcher: RouteMatcher::Exact("/".to_string()),
            grpc_match: None,
            action: RouteAction::Return(ReturnAction {
                status: StatusCode::OK,
                location: String::new(),
                body: Some("ok\n".to_string()),
            }),
            access_control: RouteAccessControl::default(),
            rate_limit: None,
        }];
        snapshot
    }

    #[test]
    fn validate_config_transition_allows_unchanged_listener() {
        let current = snapshot("127.0.0.1:8080");
        let next = snapshot("127.0.0.1:8080");

        validate_config_transition(&current, &next)
            .expect("transition should allow the same listener");
    }

    #[test]
    fn validate_config_transition_rejects_listener_change() {
        let current = snapshot("127.0.0.1:8080");
        let next = snapshot("127.0.0.1:9090");

        let error = validate_config_transition(&current, &next)
            .expect_err("transition should reject rebinding");
        assert!(error.to_string().contains("reload requires restart"));
        assert!(error.to_string().contains("default.listen"));
    }

    #[test]
    fn validate_config_transition_rejects_worker_thread_change() {
        let mut current = snapshot("127.0.0.1:8080");
        current.runtime.worker_threads = Some(2);
        let mut next = snapshot("127.0.0.1:8080");
        next.runtime.worker_threads = Some(4);

        let error = validate_config_transition(&current, &next)
            .expect_err("transition should reject worker changes");
        assert!(error.to_string().contains("runtime.worker_threads"));
    }

    #[test]
    fn validate_config_transition_rejects_accept_worker_change() {
        let mut current = snapshot("127.0.0.1:8080");
        current.runtime.accept_workers = 1;
        let mut next = snapshot("127.0.0.1:8080");
        next.runtime.accept_workers = 2;

        let error = validate_config_transition(&current, &next)
            .expect_err("transition should reject accept workers");
        assert!(error.to_string().contains("runtime.accept_workers"));
    }

    #[tokio::test]
    async fn status_snapshot_reports_runtime_summary() {
        let shared = SharedState::from_config_path(
            PathBuf::from("/etc/rginx/rginx.ron"),
            snapshot("127.0.0.1:8080"),
        )
        .expect("shared state should build");

        let status = shared.status_snapshot().await;
        assert_eq!(status.revision, 0);
        assert_eq!(status.config_path, Some(PathBuf::from("/etc/rginx/rginx.ron")));
        assert_eq!(status.listen_addr, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(status.total_vhosts, 1);
        assert_eq!(status.total_routes, 0);
        assert_eq!(status.total_upstreams, 0);
        assert!(!status.tls_enabled);
        assert_eq!(status.tls.listeners.len(), 1);
        assert_eq!(status.tls.listeners[0].session_resumption_enabled, None);
        assert_eq!(status.tls.listeners[0].session_tickets_enabled, None);
        assert_eq!(status.tls.listeners[0].session_cache_size, None);
        assert_eq!(status.tls.listeners[0].session_ticket_count, None);
        assert_eq!(status.tls.certificates.len(), 0);
        assert_eq!(status.tls.expiring_certificate_count, 0);
        assert_eq!(status.mtls.configured_listeners, 0);
        assert_eq!(status.mtls.authenticated_requests, 0);
        assert_eq!(status.active_connections, 0);
        assert_eq!(status.reload.attempts_total, 0);
    }

    #[test]
    fn counters_snapshot_tracks_connections_requests_and_response_buckets() {
        let shared = SharedState::from_config(snapshot("127.0.0.1:8080"))
            .expect("shared state should build");

        shared.record_connection_accepted("default");
        shared.record_connection_rejected("default");
        shared.record_downstream_request("default", "server", None);
        shared.record_downstream_request("default", "server", None);
        shared.record_downstream_response("default", "server", None, StatusCode::OK);
        shared.record_downstream_response("default", "server", None, StatusCode::NOT_FOUND);
        shared.record_downstream_response("default", "server", None, StatusCode::BAD_GATEWAY);

        let counters = shared.counters_snapshot();
        assert_eq!(counters.downstream_connections_accepted, 1);
        assert_eq!(counters.downstream_connections_rejected, 1);
        assert_eq!(counters.downstream_requests, 2);
        assert_eq!(counters.downstream_responses, 3);
        assert_eq!(counters.downstream_responses_2xx, 1);
        assert_eq!(counters.downstream_responses_4xx, 1);
        assert_eq!(counters.downstream_responses_5xx, 1);
        assert_eq!(counters.downstream_mtls_authenticated_requests, 0);
        assert_eq!(counters.downstream_tls_handshake_failures, 0);
    }

    #[test]
    fn counters_snapshot_tracks_mtls_activity() {
        let shared = SharedState::from_config(snapshot("127.0.0.1:8080"))
            .expect("shared state should build");

        shared.record_mtls_handshake_success("default", true);
        shared.record_mtls_request("default", true);
        shared.record_mtls_request("default", false);
        shared
            .record_tls_handshake_failure("default", TlsHandshakeFailureReason::MissingClientCert);
        shared.record_tls_handshake_failure("default", TlsHandshakeFailureReason::UnknownCa);
        shared.record_tls_handshake_failure("default", TlsHandshakeFailureReason::BadCertificate);
        shared.record_tls_handshake_failure("default", TlsHandshakeFailureReason::Other);

        let counters = shared.counters_snapshot();
        assert_eq!(counters.downstream_mtls_authenticated_connections, 1);
        assert_eq!(counters.downstream_mtls_authenticated_requests, 1);
        assert_eq!(counters.downstream_mtls_anonymous_requests, 1);
        assert_eq!(counters.downstream_tls_handshake_failures, 4);
        assert_eq!(counters.downstream_tls_handshake_failures_missing_client_cert, 1);
        assert_eq!(counters.downstream_tls_handshake_failures_unknown_ca, 1);
        assert_eq!(counters.downstream_tls_handshake_failures_bad_certificate, 1);
        assert_eq!(counters.downstream_tls_handshake_failures_other, 1);
    }

    #[tokio::test]
    async fn mtls_status_snapshot_excludes_non_mtls_listener_handshake_failures() {
        let shared = SharedState::from_config(snapshot("127.0.0.1:8080"))
            .expect("shared state should build");

        shared
            .record_tls_handshake_failure("default", TlsHandshakeFailureReason::MissingClientCert);

        let status = shared.status_snapshot().await;
        let counters = shared.counters_snapshot();

        assert_eq!(counters.downstream_tls_handshake_failures, 1);
        assert_eq!(status.mtls.configured_listeners, 0);
        assert_eq!(status.mtls.handshake_failures_total, 0);
    }

    #[test]
    fn inspect_certificate_reports_fingerprint_and_incomplete_chain_diagnostics() {
        let temp_dir = std::env::temp_dir().join("rginx-cert-inspect-test");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("temp dir should be created");
        let cert_path = temp_dir.join("leaf.crt");

        let mut ca_params = CertificateParams::default();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.distinguished_name.push(DnType::CommonName, "Test Root CA");
        let ca_key = KeyPair::generate().expect("CA key should generate");
        let _ca_cert = ca_params.self_signed(&ca_key).expect("CA should self-sign");
        let ca_issuer = Issuer::from_params(&ca_params, &ca_key);

        let mut leaf_params =
            CertificateParams::new(vec!["leaf.example.com".to_string()]).expect("leaf params");
        leaf_params.distinguished_name.push(DnType::CommonName, "leaf.example.com");
        leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let leaf_key = KeyPair::generate().expect("leaf key should generate");
        let leaf_cert =
            leaf_params.signed_by(&leaf_key, &ca_issuer).expect("leaf should be signed by CA");

        fs::write(&cert_path, leaf_cert.pem()).expect("leaf cert should be written");

        let inspected = inspect_certificate(&cert_path).expect("certificate should be inspected");
        assert_eq!(inspected.subject.as_deref(), Some("CN=leaf.example.com"));
        assert_eq!(inspected.issuer.as_deref(), Some("CN=Test Root CA"));
        assert!(!inspected.san_dns_names.is_empty());
        assert!(inspected.fingerprint_sha256.as_ref().is_some_and(|value| value.len() == 64));
        assert_eq!(inspected.chain_length, 1);
        assert!(inspected.chain_diagnostics.iter().any(|diagnostic| {
            diagnostic.contains("chain_incomplete_single_non_self_signed_certificate")
        }));

        fs::remove_dir_all(temp_dir).expect("temp dir should be removed");
    }

    #[test]
    fn inspect_certificate_reports_aki_ski_and_server_auth_eku_diagnostics() {
        let temp_dir = std::env::temp_dir().join("rginx-cert-inspect-extensions-test");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("temp dir should be created");
        let cert_path = temp_dir.join("leaf.crt");

        let mut ca_params = CertificateParams::default();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.distinguished_name.push(DnType::CommonName, "Extension Root CA");
        let ca_key = KeyPair::generate().expect("CA key should generate");
        let _ca_cert = ca_params.self_signed(&ca_key).expect("CA should self-sign");
        let ca_issuer = Issuer::from_params(&ca_params, &ca_key);

        let mut leaf_params = CertificateParams::new(vec!["client-only.example.com".to_string()])
            .expect("leaf params");
        leaf_params.distinguished_name.push(DnType::CommonName, "client-only.example.com");
        leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let leaf_key = KeyPair::generate().expect("leaf key should generate");
        let leaf_cert =
            leaf_params.signed_by(&leaf_key, &ca_issuer).expect("leaf should be signed by CA");

        fs::write(&cert_path, leaf_cert.pem()).expect("leaf cert should be written");

        let inspected = inspect_certificate(&cert_path).expect("certificate should be inspected");
        assert!(inspected.extended_key_usage.iter().any(|usage| usage == "client_auth"));
        assert!(
            inspected
                .chain_diagnostics
                .iter()
                .any(|diagnostic| diagnostic == "leaf_missing_server_auth_eku")
        );

        fs::remove_dir_all(temp_dir).expect("temp dir should be removed");
    }

    #[test]
    fn reload_status_snapshot_tracks_last_success_and_failure() {
        let shared = SharedState::from_config(snapshot("127.0.0.1:8080"))
            .expect("shared state should build");

        shared.record_reload_success(2, Vec::new());
        let first = shared.reload_status_snapshot();
        assert_eq!(first.attempts_total, 1);
        assert_eq!(first.successes_total, 1);
        assert_eq!(first.failures_total, 0);
        assert!(matches!(
            first.last_result.as_ref().map(|result| &result.outcome),
            Some(ReloadOutcomeSnapshot::Success { revision: 2 })
        ));

        shared.record_reload_failure("bad config", 2);
        let second = shared.reload_status_snapshot();
        assert_eq!(second.attempts_total, 2);
        assert_eq!(second.successes_total, 1);
        assert_eq!(second.failures_total, 1);
        assert!(matches!(
            second.last_result.as_ref().map(|result| &result.outcome),
            Some(ReloadOutcomeSnapshot::Failure { error }) if error == "bad config"
        ));
        assert_eq!(second.last_result.as_ref().map(|result| result.active_revision), Some(2));
        assert_eq!(
            second.last_result.as_ref().and_then(|result| result.rollback_preserved_revision),
            Some(2)
        );
    }

    #[test]
    fn upstream_stats_snapshot_tracks_requests_attempts_and_failovers() {
        let shared = SharedState::from_config(snapshot_with_upstream("127.0.0.1:8080"))
            .expect("shared state should build");

        shared.record_upstream_request("backend");
        shared.record_upstream_peer_attempt("backend", "http://127.0.0.1:9000");
        shared.record_upstream_peer_success("backend", "http://127.0.0.1:9000");
        shared.record_upstream_request("backend");
        shared.record_upstream_peer_attempt("backend", "http://127.0.0.1:9000");
        shared.record_upstream_peer_failure("backend", "http://127.0.0.1:9000");
        shared.record_upstream_failover("backend");
        shared.record_upstream_request("backend");
        shared.record_upstream_peer_attempt("backend", "http://127.0.0.1:9000");
        shared.record_upstream_peer_timeout("backend", "http://127.0.0.1:9000");

        let snapshot = shared.upstream_stats_snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].upstream_name, "backend");
        assert_eq!(snapshot[0].downstream_requests_total, 3);
        assert_eq!(snapshot[0].peer_attempts_total, 3);
        assert_eq!(snapshot[0].peer_successes_total, 1);
        assert_eq!(snapshot[0].peer_failures_total, 1);
        assert_eq!(snapshot[0].peer_timeouts_total, 1);
        assert_eq!(snapshot[0].failovers_total, 1);
        assert_eq!(snapshot[0].completed_responses_total, 0);
        assert_eq!(snapshot[0].bad_gateway_responses_total, 0);
        assert_eq!(snapshot[0].gateway_timeout_responses_total, 0);
        assert_eq!(snapshot[0].bad_request_responses_total, 0);
        assert_eq!(snapshot[0].payload_too_large_responses_total, 0);
        assert_eq!(snapshot[0].unsupported_media_type_responses_total, 0);
        assert_eq!(snapshot[0].no_healthy_peers_total, 0);
        assert_eq!(snapshot[0].peers.len(), 1);
        assert_eq!(snapshot[0].peers[0].peer_url, "http://127.0.0.1:9000");
        assert_eq!(snapshot[0].peers[0].attempts_total, 3);
        assert_eq!(snapshot[0].peers[0].successes_total, 1);
        assert_eq!(snapshot[0].peers[0].failures_total, 1);
        assert_eq!(snapshot[0].peers[0].timeouts_total, 1);
    }

    #[test]
    fn upstream_stats_snapshot_tracks_terminal_response_reasons() {
        let shared = SharedState::from_config(snapshot_with_upstream("127.0.0.1:8080"))
            .expect("shared state should build");

        shared.record_upstream_request("backend");
        shared.record_upstream_completed_response("backend");
        shared.record_upstream_request("backend");
        shared.record_upstream_bad_gateway_response("backend");
        shared.record_upstream_request("backend");
        shared.record_upstream_gateway_timeout_response("backend");
        shared.record_upstream_request("backend");
        shared.record_upstream_bad_request_response("backend");
        shared.record_upstream_request("backend");
        shared.record_upstream_payload_too_large_response("backend");
        shared.record_upstream_request("backend");
        shared.record_upstream_unsupported_media_type_response("backend");
        shared.record_upstream_request("backend");
        shared.record_upstream_no_healthy_peers("backend");
        shared.record_upstream_bad_gateway_response("backend");

        let snapshot = shared.upstream_stats_snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].downstream_requests_total, 7);
        assert_eq!(snapshot[0].completed_responses_total, 1);
        assert_eq!(snapshot[0].bad_gateway_responses_total, 2);
        assert_eq!(snapshot[0].gateway_timeout_responses_total, 1);
        assert_eq!(snapshot[0].bad_request_responses_total, 1);
        assert_eq!(snapshot[0].payload_too_large_responses_total, 1);
        assert_eq!(snapshot[0].unsupported_media_type_responses_total, 1);
        assert_eq!(snapshot[0].no_healthy_peers_total, 1);
        assert_eq!(snapshot[0].recent_60s.window_secs, 60);
        assert_eq!(snapshot[0].recent_60s.downstream_requests_total, 7);
        assert_eq!(snapshot[0].recent_60s.completed_responses_total, 1);
        assert_eq!(snapshot[0].recent_60s.bad_gateway_responses_total, 2);
        assert_eq!(snapshot[0].recent_60s.gateway_timeout_responses_total, 1);
    }

    #[test]
    fn traffic_stats_snapshot_tracks_listener_vhost_and_route_counters() {
        let shared = SharedState::from_config(snapshot_with_routes("127.0.0.1:8080"))
            .expect("shared state should build");

        shared.record_connection_accepted("default");
        shared.record_connection_rejected("default");
        shared.record_downstream_request("default", "server", Some("server/routes[0]|exact:/"));
        shared.record_route_access_denied("server/routes[0]|exact:/");
        shared.record_downstream_response(
            "default",
            "server",
            Some("server/routes[0]|exact:/"),
            StatusCode::FORBIDDEN,
        );
        shared.record_downstream_request("default", "server", Some("server/routes[0]|exact:/"));
        shared.record_route_rate_limited("server/routes[0]|exact:/");
        shared.record_downstream_response(
            "default",
            "server",
            Some("server/routes[0]|exact:/"),
            StatusCode::TOO_MANY_REQUESTS,
        );
        shared.record_downstream_request("default", "server", Some("server/routes[0]|exact:/"));
        shared.record_downstream_response(
            "default",
            "server",
            Some("server/routes[0]|exact:/"),
            StatusCode::OK,
        );

        let snapshot = shared.traffic_stats_snapshot();
        assert_eq!(snapshot.listeners.len(), 1);
        assert_eq!(snapshot.listeners[0].listener_id, "default");
        assert_eq!(snapshot.listeners[0].downstream_connections_accepted, 1);
        assert_eq!(snapshot.listeners[0].downstream_connections_rejected, 1);
        assert_eq!(snapshot.listeners[0].downstream_requests, 3);
        assert_eq!(snapshot.listeners[0].unmatched_requests_total, 0);
        assert_eq!(snapshot.listeners[0].downstream_responses, 3);
        assert_eq!(snapshot.listeners[0].downstream_responses_2xx, 1);
        assert_eq!(snapshot.listeners[0].downstream_responses_4xx, 2);

        assert_eq!(snapshot.vhosts.len(), 1);
        assert_eq!(snapshot.vhosts[0].vhost_id, "server");
        assert_eq!(snapshot.vhosts[0].downstream_requests, 3);
        assert_eq!(snapshot.vhosts[0].unmatched_requests_total, 0);
        assert_eq!(snapshot.vhosts[0].downstream_responses, 3);
        assert_eq!(snapshot.vhosts[0].downstream_responses_2xx, 1);
        assert_eq!(snapshot.vhosts[0].downstream_responses_4xx, 2);

        assert_eq!(snapshot.routes.len(), 1);
        assert_eq!(snapshot.routes[0].route_id, "server/routes[0]|exact:/");
        assert_eq!(snapshot.routes[0].vhost_id, "server");
        assert_eq!(snapshot.routes[0].downstream_requests, 3);
        assert_eq!(snapshot.routes[0].downstream_responses, 3);
        assert_eq!(snapshot.routes[0].downstream_responses_2xx, 1);
        assert_eq!(snapshot.routes[0].downstream_responses_4xx, 2);
        assert_eq!(snapshot.routes[0].access_denied_total, 1);
        assert_eq!(snapshot.routes[0].rate_limited_total, 1);
        assert_eq!(snapshot.listeners[0].recent_60s.window_secs, 60);
        assert_eq!(snapshot.listeners[0].recent_60s.downstream_requests_total, 3);
        assert_eq!(snapshot.listeners[0].recent_60s.downstream_responses_total, 3);
        assert_eq!(snapshot.listeners[0].recent_60s.downstream_responses_2xx_total, 1);
        assert_eq!(snapshot.listeners[0].recent_60s.downstream_responses_4xx_total, 2);
        assert_eq!(snapshot.listeners[0].recent_60s.downstream_responses_5xx_total, 0);
    }

    #[test]
    fn traffic_stats_snapshot_tracks_unmatched_requests_per_listener_and_vhost() {
        let shared = SharedState::from_config(snapshot_with_routes("127.0.0.1:8080"))
            .expect("shared state should build");

        shared.record_downstream_request("default", "server", None);
        shared.record_downstream_response("default", "server", None, StatusCode::NOT_FOUND);

        let snapshot = shared.traffic_stats_snapshot();
        assert_eq!(snapshot.listeners.len(), 1);
        assert_eq!(snapshot.listeners[0].downstream_requests, 1);
        assert_eq!(snapshot.listeners[0].unmatched_requests_total, 1);
        assert_eq!(snapshot.listeners[0].downstream_responses_4xx, 1);

        assert_eq!(snapshot.vhosts.len(), 1);
        assert_eq!(snapshot.vhosts[0].downstream_requests, 1);
        assert_eq!(snapshot.vhosts[0].unmatched_requests_total, 1);
        assert_eq!(snapshot.vhosts[0].downstream_responses_4xx, 1);

        assert_eq!(snapshot.routes.len(), 1);
        assert_eq!(snapshot.routes[0].downstream_requests, 0);
        assert_eq!(snapshot.routes[0].downstream_responses, 0);
    }

    #[test]
    fn traffic_stats_snapshot_tracks_grpc_protocols_and_statuses() {
        let shared = SharedState::from_config(snapshot_with_routes("127.0.0.1:8080"))
            .expect("shared state should build");

        shared.record_grpc_request("default", "server", Some("server/routes[0]|exact:/"), "grpc");
        shared.record_grpc_status("default", "server", Some("server/routes[0]|exact:/"), Some("0"));
        shared.record_grpc_request(
            "default",
            "server",
            Some("server/routes[0]|exact:/"),
            "grpc-web",
        );
        shared.record_grpc_status(
            "default",
            "server",
            Some("server/routes[0]|exact:/"),
            Some("14"),
        );
        shared.record_grpc_request(
            "default",
            "server",
            Some("server/routes[0]|exact:/"),
            "grpc-web-text",
        );
        shared.record_grpc_status(
            "default",
            "server",
            Some("server/routes[0]|exact:/"),
            Some("custom"),
        );

        let snapshot = shared.traffic_stats_snapshot();
        assert_eq!(snapshot.listeners[0].grpc.requests_total, 3);
        assert_eq!(snapshot.listeners[0].grpc.protocol_grpc_total, 1);
        assert_eq!(snapshot.listeners[0].grpc.protocol_grpc_web_total, 1);
        assert_eq!(snapshot.listeners[0].grpc.protocol_grpc_web_text_total, 1);
        assert_eq!(snapshot.listeners[0].grpc.status_0_total, 1);
        assert_eq!(snapshot.listeners[0].grpc.status_14_total, 1);
        assert_eq!(snapshot.listeners[0].grpc.status_other_total, 1);

        assert_eq!(snapshot.vhosts[0].grpc.requests_total, 3);
        assert_eq!(snapshot.vhosts[0].grpc.status_0_total, 1);
        assert_eq!(snapshot.vhosts[0].grpc.status_14_total, 1);
        assert_eq!(snapshot.vhosts[0].grpc.status_other_total, 1);

        assert_eq!(snapshot.routes[0].grpc.requests_total, 3);
        assert_eq!(snapshot.routes[0].grpc.status_0_total, 1);
        assert_eq!(snapshot.routes[0].grpc.status_14_total, 1);
        assert_eq!(snapshot.routes[0].grpc.status_other_total, 1);
    }
}
