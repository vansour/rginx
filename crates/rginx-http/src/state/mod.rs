use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock as StdRwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use http::StatusCode;
use rginx_core::{ConfigSnapshot, Listener, Result};
use tokio::sync::{Notify, RwLock, watch};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

use crate::cache::CacheManager;
use crate::proxy::{HealthChangeNotifier, ProxyClients, UpstreamHealthSnapshot};
use crate::rate_limit::RateLimiters;
use crate::tls::build_tls_acceptor;
use crate::tls::ocsp::ocsp_responder_urls_for_certificate;

mod connections;
mod lifecycle;
mod snapshot_bus;
mod snapshots;
#[cfg(test)]
mod tests;
mod tls_runtime;
mod traffic;
mod upstreams;

const RECENT_WINDOW_SECS: u64 = 60;
const MAX_RECENT_WINDOW_SECS: u64 = 300;
const TLS_EXPIRY_WARNING_DAYS: i64 = 30;

pub(super) struct PreparedState {
    config: Arc<ConfigSnapshot>,
    clients: ProxyClients,
    cache: CacheManager,
    listener_tls_acceptors: HashMap<String, Option<TlsAcceptor>>,
    retired_listeners: Vec<Listener>,
}

pub use snapshots::ActiveState;
pub use snapshots::{
    GrpcTrafficSnapshot, Http3ListenerRuntimeSnapshot, HttpCountersSnapshot, ListenerStatsSnapshot,
    MtlsStatusSnapshot, RecentTrafficStatsSnapshot, RecentUpstreamStatsSnapshot,
    ReloadOutcomeSnapshot, ReloadResultSnapshot, ReloadStatusSnapshot, RouteStatsSnapshot,
    RuntimeListenerBindingSnapshot, RuntimeListenerSnapshot, RuntimeStatusSnapshot,
    SnapshotDeltaSnapshot, SnapshotModule, TlsCertificateStatusSnapshot,
    TlsDefaultCertificateBindingSnapshot, TlsListenerStatusSnapshot, TlsOcspRefreshSpec,
    TlsOcspStatusSnapshot, TlsReloadBoundarySnapshot, TlsRuntimeSnapshot, TlsSniBindingSnapshot,
    TlsVhostBindingSnapshot, TrafficStatsSnapshot, UpstreamPeerStatsSnapshot,
    UpstreamStatsSnapshot, UpstreamTlsStatusSnapshot, VhostStatsSnapshot,
};
include!("counters/http.rs");
include!("counters/rolling.rs");
include!("counters/grpc.rs");
include!("counters/traffic.rs");
include!("counters/upstreams.rs");
include!("counters/versions.rs");
include!("helpers.rs");

#[cfg(test)]
pub(crate) use crate::validate_config_transition;
pub use connections::ActiveConnectionGuard;
#[cfg(test)]
pub(crate) use tls_runtime::inspect_certificate;
pub use tls_runtime::{
    tls_ocsp_refresh_specs_for_config, tls_reloadable_fields, tls_restart_required_fields,
    tls_runtime_snapshot_for_config,
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
        crate::install_default_crypto_provider();
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
                cache: prepared.cache,
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
            config_path: config_path.map(Arc::new),
        })
    }
}
