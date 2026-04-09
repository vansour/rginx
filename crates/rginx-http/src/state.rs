use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock as StdRwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use http::StatusCode;
use rginx_core::{ConfigSnapshot, Error, Listener, Result};
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
use crate::tls::certificates::{
    ocsp_responder_urls_for_certificate, validate_ocsp_response_for_certificate,
};

const RECENT_WINDOW_SECS: u64 = 60;
const MAX_RECENT_WINDOW_SECS: u64 = 300;
const TLS_EXPIRY_WARNING_DAYS: i64 = 30;

struct PreparedState {
    config: Arc<ConfigSnapshot>,
    clients: ProxyClients,
    listener_tls_acceptors: HashMap<String, Option<TlsAcceptor>>,
}

include!("state/snapshots.rs");
include!("state/counters.rs");
include!("state/helpers.rs");

#[derive(Clone)]
pub struct SharedState {
    inner: Arc<RwLock<ActiveState>>,
    revisions: watch::Sender<u64>,
    rate_limiters: RateLimiters,
    snapshot_version: Arc<AtomicU64>,
    snapshot_notify: Arc<Notify>,
    snapshot_components: Arc<SnapshotComponentVersions>,
    listener_tls_acceptors: Arc<RwLock<HashMap<String, Option<TlsAcceptor>>>>,
    listener_active_connections: Arc<HashMap<String, Arc<AtomicUsize>>>,
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

pub struct ActiveConnectionGuard {
    active_connections: Arc<AtomicUsize>,
    listener_active_connections: Arc<AtomicUsize>,
    listener_id: String,
    snapshot_version: Arc<AtomicU64>,
    snapshot_notify: Arc<Notify>,
    snapshot_components: Arc<SnapshotComponentVersions>,
    traffic_component_versions: Arc<StdRwLock<TrafficComponentVersions>>,
}

struct TrafficCounterRefs {
    listener: Option<Arc<ListenerTrafficCounters>>,
    vhost: Option<Arc<RequestTrafficCounters>>,
    route: Option<Arc<RouteTrafficCounters>>,
}

#[derive(Debug, Clone, Default)]
struct OcspRuntimeStatusEntry {
    last_refresh_unix_ms: Option<u64>,
    refreshes_total: u64,
    failures_total: u64,
    last_error: Option<String>,
}

impl Drop for ActiveConnectionGuard {
    fn drop(&mut self) {
        self.active_connections.fetch_sub(1, Ordering::AcqRel);
        self.listener_active_connections.fetch_sub(1, Ordering::AcqRel);
        let version = self.snapshot_version.fetch_add(1, Ordering::Relaxed) + 1;
        self.snapshot_components.status.store(version, Ordering::Relaxed);
        self.snapshot_components.traffic.store(version, Ordering::Relaxed);
        self.traffic_component_versions
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .listeners
            .insert(self.listener_id.clone(), version);
        self.snapshot_notify.notify_waiters();
    }
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
            listener_active_connections: Arc::new(listener_active_connections),
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

    pub async fn snapshot(&self) -> ActiveState {
        self.inner.read().await.clone()
    }

    pub async fn current_config(&self) -> Arc<ConfigSnapshot> {
        self.inner.read().await.config.clone()
    }

    pub async fn current_revision(&self) -> u64 {
        self.inner.read().await.revision
    }

    pub fn current_snapshot_version(&self) -> u64 {
        self.snapshot_version.load(Ordering::Relaxed)
    }

    pub async fn current_listener(&self, listener_id: &str) -> Option<Listener> {
        self.inner.read().await.config.listener(listener_id).cloned()
    }

    pub fn config_path(&self) -> Option<&PathBuf> {
        self.config_path.as_deref()
    }

    pub fn subscribe_updates(&self) -> watch::Receiver<u64> {
        self.revisions.subscribe()
    }

    pub fn rate_limiters(&self) -> RateLimiters {
        self.rate_limiters.clone()
    }

    pub fn active_connection_count(&self) -> usize {
        self.active_connections.load(Ordering::Acquire)
    }

    pub async fn wait_for_snapshot_change(
        &self,
        since_version: u64,
        timeout: Option<std::time::Duration>,
    ) -> u64 {
        loop {
            let notified = self.snapshot_notify.notified();
            let current = self.current_snapshot_version();
            if current > since_version {
                return current;
            }

            if let Some(timeout) = timeout {
                if tokio::time::timeout(timeout, notified).await.is_err() {
                    return self.current_snapshot_version();
                }
            } else {
                notified.await;
            }
        }
    }

    pub fn snapshot_delta_since(
        &self,
        since_version: u64,
        include: Option<&[SnapshotModule]>,
        window_secs: Option<u64>,
    ) -> SnapshotDeltaSnapshot {
        let included_modules = SnapshotModule::normalize(include);
        let current_snapshot_version = self.current_snapshot_version();
        let status_version = self.snapshot_components.status.load(Ordering::Relaxed);
        let counters_version = self.snapshot_components.counters.load(Ordering::Relaxed);
        let traffic_version = self.snapshot_components.traffic.load(Ordering::Relaxed);
        let peer_health_version = self.snapshot_components.peer_health.load(Ordering::Relaxed);
        let upstreams_version = self.snapshot_components.upstreams.load(Ordering::Relaxed);
        let (changed_listener_ids, changed_vhost_ids, changed_route_ids) =
            self.changed_traffic_targets_since(since_version);
        let changed_peer_health_upstream_names = self.changed_named_component_targets_since(
            &self.peer_health_component_versions,
            since_version,
        );
        let changed_upstream_names = self.changed_named_component_targets_since(
            &self.upstream_component_versions,
            since_version,
        );

        SnapshotDeltaSnapshot {
            schema_version: 2,
            since_version,
            current_snapshot_version,
            included_modules: included_modules.clone(),
            recent_window_secs: window_secs,
            status_version: included_modules
                .contains(&SnapshotModule::Status)
                .then_some(status_version),
            counters_version: included_modules
                .contains(&SnapshotModule::Counters)
                .then_some(counters_version),
            traffic_version: included_modules
                .contains(&SnapshotModule::Traffic)
                .then_some(traffic_version),
            peer_health_version: included_modules
                .contains(&SnapshotModule::PeerHealth)
                .then_some(peer_health_version),
            upstreams_version: included_modules
                .contains(&SnapshotModule::Upstreams)
                .then_some(upstreams_version),
            status_changed: included_modules
                .contains(&SnapshotModule::Status)
                .then_some(status_version > since_version),
            counters_changed: included_modules
                .contains(&SnapshotModule::Counters)
                .then_some(counters_version > since_version),
            traffic_changed: included_modules
                .contains(&SnapshotModule::Traffic)
                .then_some(traffic_version > since_version),
            traffic_recent_changed: (included_modules.contains(&SnapshotModule::Traffic)
                && window_secs.is_some())
            .then_some(traffic_version > since_version),
            peer_health_changed: included_modules
                .contains(&SnapshotModule::PeerHealth)
                .then_some(peer_health_version > since_version),
            upstreams_changed: included_modules
                .contains(&SnapshotModule::Upstreams)
                .then_some(upstreams_version > since_version),
            upstreams_recent_changed: (included_modules.contains(&SnapshotModule::Upstreams)
                && window_secs.is_some())
            .then_some(upstreams_version > since_version),
            changed_listener_ids: included_modules
                .contains(&SnapshotModule::Traffic)
                .then_some(changed_listener_ids.clone()),
            changed_vhost_ids: included_modules
                .contains(&SnapshotModule::Traffic)
                .then_some(changed_vhost_ids.clone()),
            changed_route_ids: included_modules
                .contains(&SnapshotModule::Traffic)
                .then_some(changed_route_ids.clone()),
            changed_recent_listener_ids: (included_modules.contains(&SnapshotModule::Traffic)
                && window_secs.is_some())
            .then_some(changed_listener_ids.clone()),
            changed_recent_vhost_ids: (included_modules.contains(&SnapshotModule::Traffic)
                && window_secs.is_some())
            .then_some(changed_vhost_ids.clone()),
            changed_recent_route_ids: (included_modules.contains(&SnapshotModule::Traffic)
                && window_secs.is_some())
            .then_some(changed_route_ids.clone()),
            changed_peer_health_upstream_names: included_modules
                .contains(&SnapshotModule::PeerHealth)
                .then_some(changed_peer_health_upstream_names.clone()),
            changed_upstream_names: included_modules
                .contains(&SnapshotModule::Upstreams)
                .then_some(changed_upstream_names.clone()),
            changed_recent_upstream_names: (included_modules.contains(&SnapshotModule::Upstreams)
                && window_secs.is_some())
            .then_some(changed_upstream_names.clone()),
        }
    }

    pub fn counters_snapshot(&self) -> HttpCountersSnapshot {
        self.counters.snapshot()
    }

    pub fn reload_status_snapshot(&self) -> ReloadStatusSnapshot {
        self.reload_history.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).snapshot()
    }

    pub async fn status_snapshot(&self) -> RuntimeStatusSnapshot {
        let state = self.inner.read().await;
        let mtls = self.mtls_status_snapshot(state.config.as_ref());
        let ocsp_statuses =
            self.ocsp_statuses.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let tls = tls_runtime_snapshot_for_config_with_ocsp_statuses(
            state.config.as_ref(),
            Some(&ocsp_statuses),
        );
        RuntimeStatusSnapshot {
            revision: state.revision,
            config_path: self.config_path.as_deref().cloned(),
            listen_addr: state.config.server.listen_addr,
            worker_threads: state.config.runtime.worker_threads,
            accept_workers: state.config.runtime.accept_workers,
            total_vhosts: state.config.total_vhost_count(),
            total_routes: state.config.total_route_count(),
            total_upstreams: state.config.upstreams.len(),
            tls_enabled: state.config.tls_enabled(),
            tls,
            mtls,
            upstream_tls: upstream_tls_status_snapshots(state.config.as_ref()),
            active_connections: self.active_connection_count(),
            reload: self.reload_status_snapshot(),
        }
    }

    pub async fn peer_health_snapshot(&self) -> Vec<UpstreamHealthSnapshot> {
        self.inner.read().await.clients.peer_health_snapshot()
    }

    pub fn upstream_stats_snapshot(&self) -> Vec<UpstreamStatsSnapshot> {
        self.upstream_stats_snapshot_with_window(None)
    }

    pub fn upstream_stats_snapshot_with_window(
        &self,
        window_secs: Option<u64>,
    ) -> Vec<UpstreamStatsSnapshot> {
        let stats = self.upstream_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut upstream_names = stats.keys().cloned().collect::<Vec<_>>();
        upstream_names.sort();

        upstream_names
            .into_iter()
            .filter_map(|upstream_name| {
                let entry = stats.get(&upstream_name)?;
                let peers = entry
                    .peer_order
                    .iter()
                    .filter_map(|peer_url| {
                        let peer = entry.peers.get(peer_url)?;
                        Some(UpstreamPeerStatsSnapshot {
                            peer_url: peer_url.clone(),
                            attempts_total: peer.attempts_total.load(Ordering::Relaxed),
                            successes_total: peer.successes_total.load(Ordering::Relaxed),
                            failures_total: peer.failures_total.load(Ordering::Relaxed),
                            timeouts_total: peer.timeouts_total.load(Ordering::Relaxed),
                        })
                    })
                    .collect::<Vec<_>>();

                Some(UpstreamStatsSnapshot {
                    upstream_name: upstream_name.clone(),
                    tls: upstream_tls_status_snapshot(entry.upstream.as_ref()),
                    downstream_requests_total: entry
                        .counters
                        .downstream_requests_total
                        .load(Ordering::Relaxed),
                    peer_attempts_total: entry.counters.peer_attempts_total.load(Ordering::Relaxed),
                    peer_successes_total: entry
                        .counters
                        .peer_successes_total
                        .load(Ordering::Relaxed),
                    peer_failures_total: entry.counters.peer_failures_total.load(Ordering::Relaxed),
                    peer_timeouts_total: entry.counters.peer_timeouts_total.load(Ordering::Relaxed),
                    failovers_total: entry.counters.failovers_total.load(Ordering::Relaxed),
                    completed_responses_total: entry
                        .counters
                        .completed_responses_total
                        .load(Ordering::Relaxed),
                    bad_gateway_responses_total: entry
                        .counters
                        .bad_gateway_responses_total
                        .load(Ordering::Relaxed),
                    gateway_timeout_responses_total: entry
                        .counters
                        .gateway_timeout_responses_total
                        .load(Ordering::Relaxed),
                    bad_request_responses_total: entry
                        .counters
                        .bad_request_responses_total
                        .load(Ordering::Relaxed),
                    payload_too_large_responses_total: entry
                        .counters
                        .payload_too_large_responses_total
                        .load(Ordering::Relaxed),
                    unsupported_media_type_responses_total: entry
                        .counters
                        .unsupported_media_type_responses_total
                        .load(Ordering::Relaxed),
                    no_healthy_peers_total: entry
                        .counters
                        .no_healthy_peers_total
                        .load(Ordering::Relaxed),
                    tls_failures_unknown_ca_total: entry
                        .counters
                        .tls_failures_unknown_ca_total
                        .load(Ordering::Relaxed),
                    tls_failures_bad_certificate_total: entry
                        .counters
                        .tls_failures_bad_certificate_total
                        .load(Ordering::Relaxed),
                    tls_failures_certificate_revoked_total: entry
                        .counters
                        .tls_failures_certificate_revoked_total
                        .load(Ordering::Relaxed),
                    tls_failures_verify_depth_exceeded_total: entry
                        .counters
                        .tls_failures_verify_depth_exceeded_total
                        .load(Ordering::Relaxed),
                    recent_60s: entry.counters.recent_60s.snapshot(),
                    recent_window: window_secs.map(|window_secs| {
                        entry.counters.recent_60s.snapshot_for_window(window_secs)
                    }),
                    peers,
                })
            })
            .collect()
    }

    pub fn traffic_stats_snapshot(&self) -> TrafficStatsSnapshot {
        self.traffic_stats_snapshot_with_window(None)
    }

    pub fn traffic_stats_snapshot_with_window(
        &self,
        window_secs: Option<u64>,
    ) -> TrafficStatsSnapshot {
        let stats = self.traffic_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());

        let listeners = stats
            .listener_order
            .iter()
            .filter_map(|listener_id| {
                let entry = stats.listeners.get(listener_id)?;
                let (
                    downstream_responses,
                    downstream_responses_1xx,
                    downstream_responses_2xx,
                    downstream_responses_3xx,
                    downstream_responses_4xx,
                    downstream_responses_5xx,
                ) = entry.counters.responses.snapshot();
                Some(ListenerStatsSnapshot {
                    listener_id: listener_id.clone(),
                    listener_name: entry.listener_name.clone(),
                    listen_addr: entry.listen_addr,
                    active_connections: self
                        .listener_active_connections
                        .get(listener_id)
                        .map(|connections| connections.load(Ordering::Acquire))
                        .unwrap_or(0),
                    downstream_connections_accepted: entry
                        .counters
                        .downstream_connections_accepted
                        .load(Ordering::Relaxed),
                    downstream_connections_rejected: entry
                        .counters
                        .downstream_connections_rejected
                        .load(Ordering::Relaxed),
                    downstream_requests: entry.counters.downstream_requests.load(Ordering::Relaxed),
                    unmatched_requests_total: entry
                        .counters
                        .unmatched_requests_total
                        .load(Ordering::Relaxed),
                    downstream_responses,
                    downstream_responses_1xx,
                    downstream_responses_2xx,
                    downstream_responses_3xx,
                    downstream_responses_4xx,
                    downstream_responses_5xx,
                    recent_60s: entry.counters.recent_60s.snapshot(),
                    recent_window: window_secs.map(|window_secs| {
                        entry.counters.recent_60s.snapshot_for_window(window_secs)
                    }),
                    grpc: entry.counters.grpc.snapshot(),
                })
            })
            .collect();

        let vhosts = stats
            .vhost_order
            .iter()
            .filter_map(|vhost_id| {
                let entry = stats.vhosts.get(vhost_id)?;
                let (
                    downstream_responses,
                    downstream_responses_1xx,
                    downstream_responses_2xx,
                    downstream_responses_3xx,
                    downstream_responses_4xx,
                    downstream_responses_5xx,
                ) = entry.counters.responses.snapshot();
                Some(VhostStatsSnapshot {
                    vhost_id: vhost_id.clone(),
                    server_names: entry.server_names.clone(),
                    downstream_requests: entry.counters.downstream_requests.load(Ordering::Relaxed),
                    unmatched_requests_total: entry
                        .counters
                        .unmatched_requests_total
                        .load(Ordering::Relaxed),
                    downstream_responses,
                    downstream_responses_1xx,
                    downstream_responses_2xx,
                    downstream_responses_3xx,
                    downstream_responses_4xx,
                    downstream_responses_5xx,
                    recent_60s: entry.counters.recent_60s.snapshot(),
                    recent_window: window_secs.map(|window_secs| {
                        entry.counters.recent_60s.snapshot_for_window(window_secs)
                    }),
                    grpc: entry.counters.grpc.snapshot(),
                })
            })
            .collect();

        let routes = stats
            .route_order
            .iter()
            .filter_map(|route_id| {
                let entry = stats.routes.get(route_id)?;
                let (
                    downstream_responses,
                    downstream_responses_1xx,
                    downstream_responses_2xx,
                    downstream_responses_3xx,
                    downstream_responses_4xx,
                    downstream_responses_5xx,
                ) = entry.counters.responses.snapshot();
                Some(RouteStatsSnapshot {
                    route_id: route_id.clone(),
                    vhost_id: entry.vhost_id.clone(),
                    downstream_requests: entry.counters.downstream_requests.load(Ordering::Relaxed),
                    downstream_responses,
                    downstream_responses_1xx,
                    downstream_responses_2xx,
                    downstream_responses_3xx,
                    downstream_responses_4xx,
                    downstream_responses_5xx,
                    access_denied_total: entry.counters.access_denied_total.load(Ordering::Relaxed),
                    rate_limited_total: entry.counters.rate_limited_total.load(Ordering::Relaxed),
                    recent_60s: entry.counters.recent_60s.snapshot(),
                    recent_window: window_secs.map(|window_secs| {
                        entry.counters.recent_60s.snapshot_for_window(window_secs)
                    }),
                    grpc: entry.counters.grpc.snapshot(),
                })
            })
            .collect();

        TrafficStatsSnapshot { listeners, vhosts, routes }
    }

    pub fn try_acquire_connection(
        &self,
        listener_id: &str,
        limit: Option<usize>,
    ) -> Option<ActiveConnectionGuard> {
        let listener_active_connections =
            self.listener_active_connections.get(listener_id)?.clone();
        loop {
            let current = listener_active_connections.load(Ordering::Acquire);
            if limit.is_some_and(|limit| current >= limit) {
                return None;
            }

            if listener_active_connections
                .compare_exchange_weak(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                self.active_connections.fetch_add(1, Ordering::AcqRel);
                return Some(ActiveConnectionGuard {
                    active_connections: self.active_connections.clone(),
                    listener_active_connections,
                    listener_id: listener_id.to_string(),
                    snapshot_version: self.snapshot_version.clone(),
                    snapshot_notify: self.snapshot_notify.clone(),
                    snapshot_components: self.snapshot_components.clone(),
                    traffic_component_versions: self.traffic_component_versions.clone(),
                });
            }
        }
    }

    pub fn retain_connection_slot(&self, listener_id: &str) -> ActiveConnectionGuard {
        let listener_active_connections = self
            .listener_active_connections
            .get(listener_id)
            .expect("listener id should exist while retaining a connection slot")
            .clone();
        listener_active_connections.fetch_add(1, Ordering::AcqRel);
        self.active_connections.fetch_add(1, Ordering::AcqRel);
        let version = self.mark_snapshot_changed_components(true, false, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
        ActiveConnectionGuard {
            active_connections: self.active_connections.clone(),
            listener_active_connections,
            listener_id: listener_id.to_string(),
            snapshot_version: self.snapshot_version.clone(),
            snapshot_notify: self.snapshot_notify.clone(),
            snapshot_components: self.snapshot_components.clone(),
            traffic_component_versions: self.traffic_component_versions.clone(),
        }
    }

    pub(crate) fn record_connection_accepted(&self, listener_id: &str) {
        self.counters.downstream_connections_accepted.fetch_add(1, Ordering::Relaxed);
        if let Some(counters) = self.listener_traffic_counters(listener_id) {
            counters.downstream_connections_accepted.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(true, true, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
    }

    pub(crate) fn record_mtls_handshake_success(&self, listener_id: &str, authenticated: bool) {
        if !authenticated {
            return;
        }

        self.counters.downstream_mtls_authenticated_connections.fetch_add(1, Ordering::Relaxed);
        if let Some(counters) = self.listener_traffic_counters(listener_id) {
            counters.downstream_mtls_authenticated_connections.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(true, true, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
    }

    pub(crate) fn record_tls_handshake_failure(
        &self,
        listener_id: &str,
        reason: TlsHandshakeFailureReason,
    ) {
        self.counters.downstream_tls_handshake_failures.fetch_add(1, Ordering::Relaxed);
        match reason {
            TlsHandshakeFailureReason::MissingClientCert => {
                self.counters
                    .downstream_tls_handshake_failures_missing_client_cert
                    .fetch_add(1, Ordering::Relaxed);
            }
            TlsHandshakeFailureReason::UnknownCa => {
                self.counters
                    .downstream_tls_handshake_failures_unknown_ca
                    .fetch_add(1, Ordering::Relaxed);
            }
            TlsHandshakeFailureReason::BadCertificate => {
                self.counters
                    .downstream_tls_handshake_failures_bad_certificate
                    .fetch_add(1, Ordering::Relaxed);
            }
            TlsHandshakeFailureReason::CertificateRevoked => {
                self.counters
                    .downstream_tls_handshake_failures_certificate_revoked
                    .fetch_add(1, Ordering::Relaxed);
            }
            TlsHandshakeFailureReason::VerifyDepthExceeded => {
                self.counters
                    .downstream_tls_handshake_failures_verify_depth_exceeded
                    .fetch_add(1, Ordering::Relaxed);
            }
            TlsHandshakeFailureReason::Other => {
                self.counters
                    .downstream_tls_handshake_failures_other
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        if let Some(counters) = self.listener_traffic_counters(listener_id) {
            counters.downstream_tls_handshake_failures.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(true, true, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
    }

    pub(crate) fn record_connection_rejected(&self, listener_id: &str) {
        self.counters.downstream_connections_rejected.fetch_add(1, Ordering::Relaxed);
        if let Some(counters) = self.listener_traffic_counters(listener_id) {
            counters.downstream_connections_rejected.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(true, true, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
    }

    pub(crate) fn record_downstream_request(
        &self,
        listener_id: &str,
        vhost_id: &str,
        route_id: Option<&str>,
    ) {
        let counters = self.traffic_counter_refs(listener_id, vhost_id, route_id);
        self.counters.downstream_requests.fetch_add(1, Ordering::Relaxed);
        if let Some(listener) = counters.listener {
            listener.downstream_requests.fetch_add(1, Ordering::Relaxed);
            listener.recent_60s.record_downstream_request();
            if route_id.is_none() {
                listener.unmatched_requests_total.fetch_add(1, Ordering::Relaxed);
            }
        }
        if let Some(vhost) = counters.vhost {
            vhost.downstream_requests.fetch_add(1, Ordering::Relaxed);
            vhost.recent_60s.record_downstream_request();
            if route_id.is_none() {
                vhost.unmatched_requests_total.fetch_add(1, Ordering::Relaxed);
            }
        }
        if let Some(route) = counters.route {
            route.downstream_requests.fetch_add(1, Ordering::Relaxed);
            route.recent_60s.record_downstream_request();
        }
        let version = self.mark_snapshot_changed_components(false, true, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), Some(vhost_id), route_id);
    }

    pub(crate) fn record_mtls_request(&self, listener_id: &str, authenticated: bool) {
        if authenticated {
            self.counters.downstream_mtls_authenticated_requests.fetch_add(1, Ordering::Relaxed);
        } else {
            self.counters.downstream_mtls_anonymous_requests.fetch_add(1, Ordering::Relaxed);
        }

        if let Some(counters) = self.listener_traffic_counters(listener_id) {
            if authenticated {
                counters.downstream_mtls_authenticated_requests.fetch_add(1, Ordering::Relaxed);
            } else {
                counters.downstream_mtls_anonymous_requests.fetch_add(1, Ordering::Relaxed);
            }
        }

        let version = self.mark_snapshot_changed_components(true, true, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
    }

    pub(crate) fn record_downstream_response(
        &self,
        listener_id: &str,
        vhost_id: &str,
        route_id: Option<&str>,
        status: StatusCode,
    ) {
        let counters = self.traffic_counter_refs(listener_id, vhost_id, route_id);
        self.counters.downstream_responses.fetch_add(1, Ordering::Relaxed);
        match status.as_u16() / 100 {
            1 => {
                self.counters.downstream_responses_1xx.fetch_add(1, Ordering::Relaxed);
            }
            2 => {
                self.counters.downstream_responses_2xx.fetch_add(1, Ordering::Relaxed);
            }
            3 => {
                self.counters.downstream_responses_3xx.fetch_add(1, Ordering::Relaxed);
            }
            4 => {
                self.counters.downstream_responses_4xx.fetch_add(1, Ordering::Relaxed);
            }
            5 => {
                self.counters.downstream_responses_5xx.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
        if let Some(listener) = counters.listener {
            listener.responses.record(status);
            listener.recent_60s.record_downstream_response(status);
        }
        if let Some(vhost) = counters.vhost {
            vhost.responses.record(status);
            vhost.recent_60s.record_downstream_response(status);
        }
        if let Some(route) = counters.route {
            route.responses.record(status);
            route.recent_60s.record_downstream_response(status);
        }
        let version = self.mark_snapshot_changed_components(false, true, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), Some(vhost_id), route_id);
    }

    pub(crate) fn record_route_access_denied(&self, route_id: &str) {
        if let Some(counters) = self.route_traffic_counters(route_id) {
            counters.access_denied_total.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(false, false, true, false, false);
        self.mark_traffic_targets_changed(version, None, None, Some(route_id));
    }

    pub(crate) fn record_route_rate_limited(&self, route_id: &str) {
        if let Some(counters) = self.route_traffic_counters(route_id) {
            counters.rate_limited_total.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(false, false, true, false, false);
        self.mark_traffic_targets_changed(version, None, None, Some(route_id));
    }

    pub(crate) fn record_grpc_request(
        &self,
        listener_id: &str,
        vhost_id: &str,
        route_id: Option<&str>,
        protocol: &str,
    ) {
        let counters = self.traffic_counter_refs(listener_id, vhost_id, route_id);
        if let Some(listener) = counters.listener {
            listener.grpc.record_request(protocol);
            listener.recent_60s.record_grpc_request();
        }
        if let Some(vhost) = counters.vhost {
            vhost.grpc.record_request(protocol);
            vhost.recent_60s.record_grpc_request();
        }
        if let Some(route) = counters.route {
            route.grpc.record_request(protocol);
            route.recent_60s.record_grpc_request();
        }
        let version = self.mark_snapshot_changed_components(false, false, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), Some(vhost_id), route_id);
    }

    pub(crate) fn record_grpc_status(
        &self,
        listener_id: &str,
        vhost_id: &str,
        route_id: Option<&str>,
        status: Option<&str>,
    ) {
        let counters = self.traffic_counter_refs(listener_id, vhost_id, route_id);
        if let Some(listener) = counters.listener {
            listener.grpc.record_status(status);
        }
        if let Some(vhost) = counters.vhost {
            vhost.grpc.record_status(status);
        }
        if let Some(route) = counters.route {
            route.grpc.record_status(status);
        }
        let version = self.mark_snapshot_changed_components(false, false, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), Some(vhost_id), route_id);
    }

    pub(crate) fn record_upstream_request(&self, upstream_name: &str) {
        let Some(counters) = self.upstream_stats_counters(upstream_name) else {
            return;
        };
        counters.downstream_requests_total.fetch_add(1, Ordering::Relaxed);
        counters.recent_60s.downstream_requests_total.increment_now();
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_peer_attempt(&self, upstream_name: &str, peer_url: &str) {
        let Some((counters, peer)) = self.upstream_stats_peer_counters(upstream_name, peer_url)
        else {
            return;
        };
        counters.peer_attempts_total.fetch_add(1, Ordering::Relaxed);
        peer.attempts_total.fetch_add(1, Ordering::Relaxed);
        counters.recent_60s.peer_attempts_total.increment_now();
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_peer_success(&self, upstream_name: &str, peer_url: &str) {
        let Some((counters, peer)) = self.upstream_stats_peer_counters(upstream_name, peer_url)
        else {
            return;
        };
        counters.peer_successes_total.fetch_add(1, Ordering::Relaxed);
        peer.successes_total.fetch_add(1, Ordering::Relaxed);
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_peer_failure(&self, upstream_name: &str, peer_url: &str) {
        let Some((counters, peer)) = self.upstream_stats_peer_counters(upstream_name, peer_url)
        else {
            return;
        };
        counters.peer_failures_total.fetch_add(1, Ordering::Relaxed);
        peer.failures_total.fetch_add(1, Ordering::Relaxed);
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_peer_timeout(&self, upstream_name: &str, peer_url: &str) {
        let Some((counters, peer)) = self.upstream_stats_peer_counters(upstream_name, peer_url)
        else {
            return;
        };
        counters.peer_timeouts_total.fetch_add(1, Ordering::Relaxed);
        peer.timeouts_total.fetch_add(1, Ordering::Relaxed);
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_failover(&self, upstream_name: &str) {
        let Some(counters) = self.upstream_stats_counters(upstream_name) else {
            return;
        };
        counters.failovers_total.fetch_add(1, Ordering::Relaxed);
        counters.recent_60s.failovers_total.increment_now();
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_completed_response(&self, upstream_name: &str) {
        let Some(counters) = self.upstream_stats_counters(upstream_name) else {
            return;
        };
        counters.completed_responses_total.fetch_add(1, Ordering::Relaxed);
        counters.recent_60s.completed_responses_total.increment_now();
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_bad_gateway_response(&self, upstream_name: &str) {
        let Some(counters) = self.upstream_stats_counters(upstream_name) else {
            return;
        };
        counters.bad_gateway_responses_total.fetch_add(1, Ordering::Relaxed);
        counters.recent_60s.bad_gateway_responses_total.increment_now();
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_gateway_timeout_response(&self, upstream_name: &str) {
        let Some(counters) = self.upstream_stats_counters(upstream_name) else {
            return;
        };
        counters.gateway_timeout_responses_total.fetch_add(1, Ordering::Relaxed);
        counters.recent_60s.gateway_timeout_responses_total.increment_now();
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_bad_request_response(&self, upstream_name: &str) {
        let Some(counters) = self.upstream_stats_counters(upstream_name) else {
            return;
        };
        counters.bad_request_responses_total.fetch_add(1, Ordering::Relaxed);
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_payload_too_large_response(&self, upstream_name: &str) {
        let Some(counters) = self.upstream_stats_counters(upstream_name) else {
            return;
        };
        counters.payload_too_large_responses_total.fetch_add(1, Ordering::Relaxed);
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_unsupported_media_type_response(&self, upstream_name: &str) {
        let Some(counters) = self.upstream_stats_counters(upstream_name) else {
            return;
        };
        counters.unsupported_media_type_responses_total.fetch_add(1, Ordering::Relaxed);
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_no_healthy_peers(&self, upstream_name: &str) {
        let Some(counters) = self.upstream_stats_counters(upstream_name) else {
            return;
        };
        counters.no_healthy_peers_total.fetch_add(1, Ordering::Relaxed);
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub(crate) fn record_upstream_peer_failure_class(
        &self,
        upstream_name: &str,
        failure_class: &str,
    ) {
        let Some(counters) = self.upstream_stats_counters(upstream_name) else {
            return;
        };
        match failure_class {
            "unknown_ca" => {
                counters.tls_failures_unknown_ca_total.fetch_add(1, Ordering::Relaxed);
            }
            "bad_certificate" => {
                counters.tls_failures_bad_certificate_total.fetch_add(1, Ordering::Relaxed);
            }
            "certificate_revoked" => {
                counters.tls_failures_certificate_revoked_total.fetch_add(1, Ordering::Relaxed);
            }
            "verify_depth_exceeded" => {
                counters.tls_failures_verify_depth_exceeded_total.fetch_add(1, Ordering::Relaxed);
            }
            _ => return,
        }
        let version = self.mark_snapshot_changed_components(false, false, false, false, true);
        self.mark_named_component_target_changed(
            &self.upstream_component_versions,
            upstream_name,
            version,
        );
    }

    pub fn record_reload_success(&self, revision: u64, tls_certificate_changes: Vec<String>) {
        let mut history =
            self.reload_history.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        history.attempts_total += 1;
        history.successes_total += 1;
        history.last_result = Some(ReloadResultSnapshot {
            finished_at_unix_ms: unix_time_ms(SystemTime::now()),
            outcome: ReloadOutcomeSnapshot::Success { revision },
            tls_certificate_changes,
            active_revision: revision,
            rollback_preserved_revision: None,
        });
        self.mark_snapshot_changed_components(true, false, false, false, false);
    }

    pub fn record_reload_failure(&self, error: impl Into<String>, active_revision: u64) {
        let mut history =
            self.reload_history.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        history.attempts_total += 1;
        history.failures_total += 1;
        history.last_result = Some(ReloadResultSnapshot {
            finished_at_unix_ms: unix_time_ms(SystemTime::now()),
            outcome: ReloadOutcomeSnapshot::Failure { error: error.into() },
            tls_certificate_changes: Vec::new(),
            active_revision,
            rollback_preserved_revision: Some(active_revision),
        });
        self.mark_snapshot_changed_components(true, false, false, false, false);
    }

    pub fn record_ocsp_refresh_success(&self, scope: &str) {
        let mut statuses =
            self.ocsp_statuses.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = statuses.entry(scope.to_string()).or_default();
        entry.last_refresh_unix_ms = Some(unix_time_ms(SystemTime::now()));
        entry.refreshes_total += 1;
        entry.last_error = None;
        self.mark_snapshot_changed_components(true, false, false, false, false);
    }

    pub fn record_ocsp_refresh_failure(&self, scope: &str, error: impl Into<String>) {
        let mut statuses =
            self.ocsp_statuses.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = statuses.entry(scope.to_string()).or_default();
        entry.failures_total += 1;
        entry.last_error = Some(error.into());
        self.mark_snapshot_changed_components(true, false, false, false, false);
    }

    pub async fn tls_acceptor(&self, listener_id: &str) -> Option<TlsAcceptor> {
        self.listener_tls_acceptors.read().await.get(listener_id).cloned().flatten()
    }

    pub async fn replace(&self, config: ConfigSnapshot) -> Result<Arc<ConfigSnapshot>> {
        let prepared = self.prepare_replacement(config).await?;
        Ok(self.commit_prepared(prepared).await)
    }

    pub async fn refresh_tls_acceptors_from_current_config(&self) -> Result<()> {
        let config = self.current_config().await;
        let listener_tls_acceptors = prepare_listener_tls_acceptors(config.as_ref())?;
        *self.listener_tls_acceptors.write().await = listener_tls_acceptors;
        self.mark_snapshot_changed_components(true, false, false, false, false);
        Ok(())
    }

    async fn prepare_replacement(&self, config: ConfigSnapshot) -> Result<PreparedState> {
        let current = self.current_config().await;
        validate_config_transition(current.as_ref(), &config)?;
        prepare_state(
            config,
            Some(build_peer_health_notifier(
                self.snapshot_version.clone(),
                self.snapshot_notify.clone(),
                self.snapshot_components.clone(),
                self.peer_health_component_versions.clone(),
            )),
        )
    }

    async fn commit_prepared(&self, prepared: PreparedState) -> Arc<ConfigSnapshot> {
        self.sync_traffic_stats(prepared.config.as_ref());
        self.sync_peer_health_versions(prepared.config.as_ref());
        self.sync_upstream_stats(prepared.config.as_ref());

        let next_revision = {
            let mut state = self.inner.write().await;
            let next_revision = state.revision + 1;
            state.revision = next_revision;
            state.config = prepared.config.clone();
            state.clients = prepared.clients;
            next_revision
        };

        *self.listener_tls_acceptors.write().await = prepared.listener_tls_acceptors;
        let _ = self.revisions.send(next_revision);
        self.mark_snapshot_changed_components(true, false, true, true, true);

        prepared.config
    }

    pub fn spawn_background_task<F>(&self, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let handle = tokio::spawn(task);
        let mut tasks =
            self.background_tasks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        tasks.retain(|task| !task.is_finished());
        tasks.push(handle);
    }

    pub fn next_request_id(&self) -> String {
        let next = self.request_ids.fetch_add(1, Ordering::Relaxed);
        format!("rginx-{next:016x}")
    }

    fn mtls_status_snapshot(&self, config: &ConfigSnapshot) -> MtlsStatusSnapshot {
        let mut configured_listeners = 0usize;
        let mut optional_listeners = 0usize;
        let mut required_listeners = 0usize;
        let mut authenticated_connections = 0u64;
        let mut authenticated_requests = 0u64;
        let mut anonymous_requests = 0u64;
        let mut handshake_failures_total = 0u64;

        for listener in &config.listeners {
            let Some(client_auth) =
                listener.server.tls.as_ref().and_then(|tls| tls.client_auth.as_ref())
            else {
                continue;
            };
            configured_listeners += 1;
            match client_auth.mode {
                rginx_core::ServerClientAuthMode::Optional => optional_listeners += 1,
                rginx_core::ServerClientAuthMode::Required => required_listeners += 1,
            }

            if let Some(counters) = self.listener_traffic_counters(&listener.id) {
                authenticated_connections +=
                    counters.downstream_mtls_authenticated_connections.load(Ordering::Relaxed);
                authenticated_requests +=
                    counters.downstream_mtls_authenticated_requests.load(Ordering::Relaxed);
                anonymous_requests +=
                    counters.downstream_mtls_anonymous_requests.load(Ordering::Relaxed);
                handshake_failures_total +=
                    counters.downstream_tls_handshake_failures.load(Ordering::Relaxed);
            }
        }

        let counters = self.counters_snapshot();
        MtlsStatusSnapshot {
            configured_listeners,
            optional_listeners,
            required_listeners,
            authenticated_connections,
            authenticated_requests,
            anonymous_requests,
            handshake_failures_total,
            handshake_failures_missing_client_cert: counters
                .downstream_tls_handshake_failures_missing_client_cert,
            handshake_failures_unknown_ca: counters.downstream_tls_handshake_failures_unknown_ca,
            handshake_failures_bad_certificate: counters
                .downstream_tls_handshake_failures_bad_certificate,
            handshake_failures_certificate_revoked: counters
                .downstream_tls_handshake_failures_certificate_revoked,
            handshake_failures_verify_depth_exceeded: counters
                .downstream_tls_handshake_failures_verify_depth_exceeded,
            handshake_failures_other: counters.downstream_tls_handshake_failures_other,
        }
    }

    pub async fn drain_background_tasks(&self) {
        for task in take_background_tasks(&self.background_tasks) {
            if let Err(error) = task.await {
                if error.is_panic() {
                    tracing::warn!(%error, "background task panicked");
                } else if !error.is_cancelled() {
                    tracing::warn!(%error, "background task failed to join");
                }
            }
        }
    }

    pub async fn abort_background_tasks(&self) {
        let tasks = take_background_tasks(&self.background_tasks);
        for task in &tasks {
            task.abort();
        }

        for task in tasks {
            if let Err(error) = task.await
                && !error.is_cancelled()
            {
                tracing::warn!(%error, "background task failed after abort");
            }
        }
    }

    fn sync_upstream_stats(&self, config: &ConfigSnapshot) {
        let existing = self.upstream_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let next = build_upstream_stats_map(config, Some(&*existing));
        drop(existing);
        *self.upstream_stats.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
        let existing = self
            .upstream_component_versions
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let next = build_upstream_name_versions(config, Some(&*existing));
        drop(existing);
        *self
            .upstream_component_versions
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
    }

    fn sync_traffic_stats(&self, config: &ConfigSnapshot) {
        let existing = self.traffic_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let next = build_traffic_stats_index(config, Some(&*existing));
        drop(existing);
        *self.traffic_stats.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
        let existing =
            self.traffic_component_versions.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let next = build_traffic_component_versions(config, Some(&*existing));
        drop(existing);
        *self.traffic_component_versions.write().unwrap_or_else(|poisoned| poisoned.into_inner()) =
            next;
    }

    fn sync_peer_health_versions(&self, config: &ConfigSnapshot) {
        let existing = self
            .peer_health_component_versions
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let next = build_upstream_name_versions(config, Some(&*existing));
        drop(existing);
        *self
            .peer_health_component_versions
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
    }

    fn listener_traffic_counters(&self, listener_id: &str) -> Option<Arc<ListenerTrafficCounters>> {
        let stats = self.traffic_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        stats.listeners.get(listener_id).map(|entry| entry.counters.clone())
    }

    fn route_traffic_counters(&self, route_id: &str) -> Option<Arc<RouteTrafficCounters>> {
        let stats = self.traffic_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        stats.routes.get(route_id).map(|entry| entry.counters.clone())
    }

    fn traffic_counter_refs(
        &self,
        listener_id: &str,
        vhost_id: &str,
        route_id: Option<&str>,
    ) -> TrafficCounterRefs {
        let stats = self.traffic_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        TrafficCounterRefs {
            listener: stats.listeners.get(listener_id).map(|entry| entry.counters.clone()),
            vhost: stats.vhosts.get(vhost_id).map(|entry| entry.counters.clone()),
            route: route_id.and_then(|route_id| {
                stats.routes.get(route_id).map(|entry| entry.counters.clone())
            }),
        }
    }

    fn upstream_stats_counters(&self, upstream_name: &str) -> Option<Arc<UpstreamStats>> {
        let stats = self.upstream_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        stats.get(upstream_name).map(|entry| entry.counters.clone())
    }

    fn upstream_stats_peer_counters(
        &self,
        upstream_name: &str,
        peer_url: &str,
    ) -> Option<(Arc<UpstreamStats>, Arc<UpstreamPeerStats>)> {
        let stats = self.upstream_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = stats.get(upstream_name)?;
        let peer = entry.peers.get(peer_url)?.clone();
        Some((entry.counters.clone(), peer))
    }

    fn changed_traffic_targets_since(
        &self,
        since_version: u64,
    ) -> (Vec<String>, Vec<String>, Vec<String>) {
        let versions =
            self.traffic_component_versions.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut listeners = versions
            .listeners
            .iter()
            .filter_map(|(id, version)| (*version > since_version).then_some(id.clone()))
            .collect::<Vec<_>>();
        listeners.sort();
        let mut vhosts = versions
            .vhosts
            .iter()
            .filter_map(|(id, version)| (*version > since_version).then_some(id.clone()))
            .collect::<Vec<_>>();
        vhosts.sort();
        let mut routes = versions
            .routes
            .iter()
            .filter_map(|(id, version)| (*version > since_version).then_some(id.clone()))
            .collect::<Vec<_>>();
        routes.sort();
        (listeners, vhosts, routes)
    }

    fn changed_named_component_targets_since(
        &self,
        versions: &Arc<StdRwLock<HashMap<String, u64>>>,
        since_version: u64,
    ) -> Vec<String> {
        let versions = versions.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut changed = versions
            .iter()
            .filter_map(|(name, version)| (*version > since_version).then_some(name.clone()))
            .collect::<Vec<_>>();
        changed.sort();
        changed
    }

    fn mark_traffic_targets_changed(
        &self,
        version: u64,
        listener_id: Option<&str>,
        vhost_id: Option<&str>,
        route_id: Option<&str>,
    ) {
        let mut versions = self
            .traffic_component_versions
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(listener_id) = listener_id {
            versions.listeners.insert(listener_id.to_string(), version);
        }
        if let Some(vhost_id) = vhost_id {
            versions.vhosts.insert(vhost_id.to_string(), version);
        }
        if let Some(route_id) = route_id {
            versions.routes.insert(route_id.to_string(), version);
        }
    }

    fn mark_named_component_target_changed(
        &self,
        versions: &Arc<StdRwLock<HashMap<String, u64>>>,
        name: &str,
        version: u64,
    ) {
        versions
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(name.to_string(), version);
    }

    fn mark_snapshot_changed_components(
        &self,
        status: bool,
        counters: bool,
        traffic: bool,
        peer_health: bool,
        upstreams: bool,
    ) -> u64 {
        let version = self.snapshot_version.fetch_add(1, Ordering::Relaxed) + 1;
        if status {
            self.snapshot_components.status.store(version, Ordering::Relaxed);
        }
        if counters {
            self.snapshot_components.counters.store(version, Ordering::Relaxed);
        }
        if traffic {
            self.snapshot_components.traffic.store(version, Ordering::Relaxed);
        }
        if peer_health {
            self.snapshot_components.peer_health.store(version, Ordering::Relaxed);
        }
        if upstreams {
            self.snapshot_components.upstreams.store(version, Ordering::Relaxed);
        }
        self.snapshot_notify.notify_waiters();
        version
    }
}

pub fn tls_runtime_snapshot_for_config(config: &ConfigSnapshot) -> TlsRuntimeSnapshot {
    tls_runtime_snapshot_for_config_with_ocsp_statuses(config, None)
}

fn tls_runtime_snapshot_for_config_with_ocsp_statuses(
    config: &ConfigSnapshot,
    ocsp_statuses: Option<&HashMap<String, OcspRuntimeStatusEntry>>,
) -> TlsRuntimeSnapshot {
    let listeners = config
        .listeners
        .iter()
        .map(|listener| {
            let sni_names = tls_listener_sni_names(config, listener.tls_enabled());
            let tls = listener.server.tls.as_ref();
            TlsListenerStatusSnapshot {
                listener_id: listener.id.clone(),
                listener_name: listener.name.clone(),
                listen_addr: listener.server.listen_addr,
                tls_enabled: listener.tls_enabled(),
                default_certificate: listener.server.default_certificate.clone(),
                versions: tls.and_then(|tls| {
                    tls.versions.as_ref().map(|versions| {
                        versions
                            .iter()
                            .map(|version| tls_version_label(*version).to_string())
                            .collect()
                    })
                }),
                alpn_protocols: tls
                    .and_then(|tls| tls.alpn_protocols.clone())
                    .unwrap_or_else(|| vec!["h2".to_string(), "http/1.1".to_string()]),
                session_resumption_enabled: tls.map(|tls| tls.session_resumption != Some(false)),
                session_tickets_enabled: tls.map(|tls| {
                    tls.session_resumption != Some(false) && tls.session_tickets != Some(false)
                }),
                session_cache_size: tls.map(|tls| {
                    if tls.session_resumption == Some(false) {
                        0
                    } else {
                        tls.session_cache_size.unwrap_or(256)
                    }
                }),
                session_ticket_count: tls.map(|tls| {
                    if tls.session_resumption == Some(false) || tls.session_tickets == Some(false) {
                        0
                    } else {
                        tls.session_ticket_count.unwrap_or(2)
                    }
                }),
                client_auth_mode: tls.and_then(|tls| {
                    tls.client_auth.as_ref().map(|client_auth| match client_auth.mode {
                        rginx_core::ServerClientAuthMode::Optional => "optional".to_string(),
                        rginx_core::ServerClientAuthMode::Required => "required".to_string(),
                    })
                }),
                client_auth_verify_depth: tls
                    .and_then(|tls| tls.client_auth.as_ref())
                    .and_then(|client_auth| client_auth.verify_depth),
                client_auth_crl_configured: tls
                    .and_then(|tls| tls.client_auth.as_ref())
                    .and_then(|client_auth| client_auth.crl_path.as_ref())
                    .is_some(),
                sni_names,
            }
        })
        .collect::<Vec<_>>();

    let mut certificates = Vec::new();
    for listener in &config.listeners {
        if let Some(tls) = listener.server.tls.as_ref() {
            certificates.push(build_listener_certificate_snapshot(config, listener, tls));
        }
    }
    if let Some(snapshot) = build_vhost_certificate_snapshot(config, &config.default_vhost) {
        certificates.push(snapshot);
    }
    certificates.extend(
        config.vhosts.iter().filter_map(|vhost| build_vhost_certificate_snapshot(config, vhost)),
    );

    let expiring_certificate_count = certificates
        .iter()
        .filter(|certificate| {
            certificate.expires_in_days.is_some_and(|days| days <= TLS_EXPIRY_WARNING_DAYS)
        })
        .count();
    let ocsp = tls_ocsp_status_snapshots(config, ocsp_statuses);
    let (vhost_bindings, sni_bindings, sni_conflicts, default_certificate_bindings) =
        tls_binding_snapshots(config, &certificates);

    TlsRuntimeSnapshot {
        listeners,
        certificates,
        ocsp,
        vhost_bindings,
        sni_bindings,
        sni_conflicts,
        default_certificate_bindings,
        reload_boundary: TlsReloadBoundarySnapshot {
            reloadable_fields: tls_reloadable_fields(),
            restart_required_fields: tls_restart_required_fields(),
        },
        expiring_certificate_count,
    }
}

fn upstream_tls_status_snapshots(config: &ConfigSnapshot) -> Vec<UpstreamTlsStatusSnapshot> {
    let mut upstreams = config.upstreams.values().collect::<Vec<_>>();
    upstreams.sort_by(|left, right| left.name.cmp(&right.name));
    upstreams.into_iter().map(|upstream| upstream_tls_status_snapshot(upstream.as_ref())).collect()
}

fn upstream_tls_status_snapshot(upstream: &rginx_core::Upstream) -> UpstreamTlsStatusSnapshot {
    UpstreamTlsStatusSnapshot {
        upstream_name: upstream.name.clone(),
        protocol: upstream.protocol.as_str().to_string(),
        verify_mode: crate::proxy::upstream_tls_verify_label(&upstream.tls).to_string(),
        tls_versions: upstream.tls_versions.as_ref().map(|versions| {
            versions
                .iter()
                .map(|version| match version {
                    rginx_core::TlsVersion::Tls12 => "TLS1.2".to_string(),
                    rginx_core::TlsVersion::Tls13 => "TLS1.3".to_string(),
                })
                .collect()
        }),
        server_name_enabled: upstream.server_name,
        server_name_override: upstream.server_name_override.clone(),
        verify_depth: upstream.server_verify_depth,
        crl_configured: upstream.server_crl_path.is_some(),
        client_identity_configured: upstream.client_identity.is_some(),
    }
}

pub fn tls_reloadable_fields() -> Vec<String> {
    vec![
        "server.tls".to_string(),
        "listeners[].tls".to_string(),
        "servers[].tls".to_string(),
        "upstreams[].tls".to_string(),
        "upstreams[].server_name".to_string(),
        "upstreams[].server_name_override".to_string(),
    ]
}

pub fn tls_restart_required_fields() -> Vec<String> {
    vec![
        "listen".to_string(),
        "listeners".to_string(),
        "runtime.worker_threads".to_string(),
        "runtime.accept_workers".to_string(),
    ]
}

fn tls_listener_sni_names(config: &ConfigSnapshot, listener_has_tls: bool) -> Vec<String> {
    if !listener_has_tls && !config.vhosts.iter().any(|vhost| vhost.tls.is_some()) {
        return Vec::new();
    }

    let mut names = config.default_vhost.server_names.clone();
    for vhost in &config.vhosts {
        if vhost.tls.is_some() {
            names.extend(vhost.server_names.clone());
        }
    }
    names.sort();
    names.dedup();
    names
}

fn build_listener_certificate_snapshot(
    config: &ConfigSnapshot,
    listener: &Listener,
    tls: &rginx_core::ServerTls,
) -> TlsCertificateStatusSnapshot {
    let inspected = inspect_certificate(&tls.cert_path);
    TlsCertificateStatusSnapshot {
        scope: format!("listener:{}", listener.name),
        cert_path: tls.cert_path.clone(),
        server_names: config.default_vhost.server_names.clone(),
        subject: inspected.as_ref().and_then(|certificate| certificate.subject.clone()),
        issuer: inspected.as_ref().and_then(|certificate| certificate.issuer.clone()),
        serial_number: inspected.as_ref().and_then(|certificate| certificate.serial_number.clone()),
        san_dns_names: inspected
            .as_ref()
            .map(|certificate| certificate.san_dns_names.clone())
            .unwrap_or_default(),
        fingerprint_sha256: inspected
            .as_ref()
            .and_then(|certificate| certificate.fingerprint_sha256.clone()),
        subject_key_identifier: inspected
            .as_ref()
            .and_then(|certificate| certificate.subject_key_identifier.clone()),
        authority_key_identifier: inspected
            .as_ref()
            .and_then(|certificate| certificate.authority_key_identifier.clone()),
        is_ca: inspected.as_ref().and_then(|certificate| certificate.is_ca),
        path_len_constraint: inspected
            .as_ref()
            .and_then(|certificate| certificate.path_len_constraint),
        key_usage: inspected.as_ref().and_then(|certificate| certificate.key_usage.clone()),
        extended_key_usage: inspected
            .as_ref()
            .map(|certificate| certificate.extended_key_usage.clone())
            .unwrap_or_default(),
        not_before_unix_ms: inspected
            .as_ref()
            .and_then(|certificate| certificate.not_before_unix_ms),
        not_after_unix_ms: inspected.as_ref().and_then(|certificate| certificate.not_after_unix_ms),
        expires_in_days: inspected.as_ref().and_then(|certificate| certificate.expires_in_days),
        chain_length: inspected.as_ref().map(|certificate| certificate.chain_length).unwrap_or(0),
        chain_subjects: inspected
            .as_ref()
            .map(|certificate| certificate.chain_subjects.clone())
            .unwrap_or_default(),
        chain_diagnostics: inspected
            .as_ref()
            .map(|certificate| certificate.chain_diagnostics.clone())
            .unwrap_or_default(),
        selected_as_default_for_listeners: if listener.server.default_certificate.is_none()
            || listener.server.default_certificate.as_ref().is_some_and(|default_name| {
                config.default_vhost.server_names.iter().any(|name| name == default_name)
            }) {
            vec![listener.name.clone()]
        } else {
            Vec::new()
        },
        ocsp_staple_configured: tls.ocsp_staple_path.is_some(),
        additional_certificate_count: tls.additional_certificates.len(),
    }
}

fn build_vhost_certificate_snapshot(
    config: &ConfigSnapshot,
    vhost: &rginx_core::VirtualHost,
) -> Option<TlsCertificateStatusSnapshot> {
    let tls = vhost.tls.as_ref()?;
    let inspected = inspect_certificate(&tls.cert_path);
    Some(TlsCertificateStatusSnapshot {
        scope: format!("vhost:{}", vhost.id),
        cert_path: tls.cert_path.clone(),
        server_names: vhost.server_names.clone(),
        subject: inspected.as_ref().and_then(|certificate| certificate.subject.clone()),
        issuer: inspected.as_ref().and_then(|certificate| certificate.issuer.clone()),
        serial_number: inspected.as_ref().and_then(|certificate| certificate.serial_number.clone()),
        san_dns_names: inspected
            .as_ref()
            .map(|certificate| certificate.san_dns_names.clone())
            .unwrap_or_default(),
        fingerprint_sha256: inspected
            .as_ref()
            .and_then(|certificate| certificate.fingerprint_sha256.clone()),
        subject_key_identifier: inspected
            .as_ref()
            .and_then(|certificate| certificate.subject_key_identifier.clone()),
        authority_key_identifier: inspected
            .as_ref()
            .and_then(|certificate| certificate.authority_key_identifier.clone()),
        is_ca: inspected.as_ref().and_then(|certificate| certificate.is_ca),
        path_len_constraint: inspected
            .as_ref()
            .and_then(|certificate| certificate.path_len_constraint),
        key_usage: inspected.as_ref().and_then(|certificate| certificate.key_usage.clone()),
        extended_key_usage: inspected
            .as_ref()
            .map(|certificate| certificate.extended_key_usage.clone())
            .unwrap_or_default(),
        not_before_unix_ms: inspected
            .as_ref()
            .and_then(|certificate| certificate.not_before_unix_ms),
        not_after_unix_ms: inspected.as_ref().and_then(|certificate| certificate.not_after_unix_ms),
        expires_in_days: inspected.as_ref().and_then(|certificate| certificate.expires_in_days),
        chain_length: inspected.as_ref().map(|certificate| certificate.chain_length).unwrap_or(0),
        chain_subjects: inspected
            .as_ref()
            .map(|certificate| certificate.chain_subjects.clone())
            .unwrap_or_default(),
        chain_diagnostics: inspected
            .as_ref()
            .map(|certificate| certificate.chain_diagnostics.clone())
            .unwrap_or_default(),
        selected_as_default_for_listeners: config
            .listeners
            .iter()
            .filter_map(|listener| {
                listener
                    .server
                    .default_certificate
                    .as_ref()
                    .filter(|default_name| {
                        vhost.server_names.iter().any(|name| name == *default_name)
                    })
                    .map(|_| listener.name.clone())
            })
            .collect(),
        ocsp_staple_configured: tls.ocsp_staple_path.is_some(),
        additional_certificate_count: tls.additional_certificates.len(),
    })
}

fn tls_binding_snapshots(
    config: &ConfigSnapshot,
    certificates: &[TlsCertificateStatusSnapshot],
) -> (
    Vec<TlsVhostBindingSnapshot>,
    Vec<TlsSniBindingSnapshot>,
    Vec<TlsSniBindingSnapshot>,
    Vec<TlsDefaultCertificateBindingSnapshot>,
) {
    let fingerprint_by_scope = certificates
        .iter()
        .map(|certificate| {
            (
                certificate.scope.clone(),
                certificate.fingerprint_sha256.clone().unwrap_or_else(|| "-".to_string()),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();

    let mut vhost_bindings = Vec::new();
    let mut sni_bindings =
        std::collections::BTreeMap::<(String, String), TlsSniBindingSnapshot>::new();
    let mut default_certificate_bindings = Vec::new();

    for listener in &config.listeners {
        if !listener.tls_enabled() {
            continue;
        }

        for vhost in std::iter::once(&config.default_vhost).chain(config.vhosts.iter()) {
            let Some(certificate_scope) = tls_certificate_scope_for_listener_vhost(listener, vhost)
            else {
                continue;
            };
            let fingerprint = fingerprint_by_scope
                .get(&certificate_scope)
                .cloned()
                .unwrap_or_else(|| "-".to_string());
            let default_selected = listener.server.default_certificate.is_none()
                || listener.server.default_certificate.as_ref().is_some_and(|default_name| {
                    vhost.server_names.iter().any(|name| name == default_name)
                });
            vhost_bindings.push(TlsVhostBindingSnapshot {
                listener_name: listener.name.clone(),
                vhost_id: vhost.id.clone(),
                server_names: vhost.server_names.clone(),
                certificate_scopes: vec![certificate_scope.clone()],
                fingerprints: vec![fingerprint.clone()],
                default_selected,
            });

            for server_name in &vhost.server_names {
                let binding = sni_bindings
                    .entry((listener.name.clone(), server_name.clone()))
                    .or_insert_with(|| TlsSniBindingSnapshot {
                        listener_name: listener.name.clone(),
                        server_name: server_name.clone(),
                        certificate_scopes: Vec::new(),
                        fingerprints: Vec::new(),
                        default_selected,
                    });
                if !binding.certificate_scopes.iter().any(|scope| scope == &certificate_scope) {
                    binding.certificate_scopes.push(certificate_scope.clone());
                }
                if !binding.fingerprints.iter().any(|value| value == &fingerprint) {
                    binding.fingerprints.push(fingerprint.clone());
                }
                binding.default_selected = binding.default_selected || default_selected;
            }
        }

        let Some(default_certificate) = listener.server.default_certificate.as_ref() else {
            continue;
        };
        let Some(vhost) =
            std::iter::once(&config.default_vhost).chain(config.vhosts.iter()).find(|vhost| {
                vhost.server_names.iter().any(|server_name| server_name == default_certificate)
            })
        else {
            continue;
        };
        let Some(certificate_scope) = tls_certificate_scope_for_listener_vhost(listener, vhost)
        else {
            continue;
        };
        let fingerprint = fingerprint_by_scope
            .get(&certificate_scope)
            .cloned()
            .unwrap_or_else(|| "-".to_string());
        default_certificate_bindings.push(TlsDefaultCertificateBindingSnapshot {
            listener_name: listener.name.clone(),
            server_name: default_certificate.clone(),
            certificate_scopes: vec![certificate_scope],
            fingerprints: vec![fingerprint],
        });
    }

    vhost_bindings.sort_by(|left, right| {
        left.listener_name
            .cmp(&right.listener_name)
            .then_with(|| left.vhost_id.cmp(&right.vhost_id))
    });
    let mut sni_bindings = sni_bindings.into_values().collect::<Vec<_>>();
    sni_bindings.sort_by(|left, right| {
        left.listener_name
            .cmp(&right.listener_name)
            .then_with(|| left.server_name.cmp(&right.server_name))
    });
    let sni_conflicts = sni_bindings
        .iter()
        .filter(|binding| binding.fingerprints.len() > 1)
        .cloned()
        .collect::<Vec<_>>();
    default_certificate_bindings.sort_by(|left, right| {
        left.listener_name
            .cmp(&right.listener_name)
            .then_with(|| left.server_name.cmp(&right.server_name))
    });

    (vhost_bindings, sni_bindings, sni_conflicts, default_certificate_bindings)
}

fn tls_certificate_scope_for_listener_vhost(
    listener: &Listener,
    vhost: &rginx_core::VirtualHost,
) -> Option<String> {
    if vhost.tls.is_some() {
        Some(format!("vhost:{}", vhost.id))
    } else if listener.server.tls.is_some() {
        Some(format!("listener:{}", listener.name))
    } else {
        None
    }
}

#[derive(Debug, Clone)]
struct TlsOcspBundleSpec {
    scope: String,
    cert_path: PathBuf,
    ocsp_staple_path: Option<PathBuf>,
}

fn tls_ocsp_status_snapshots(
    config: &ConfigSnapshot,
    runtime_statuses: Option<&HashMap<String, OcspRuntimeStatusEntry>>,
) -> Vec<TlsOcspStatusSnapshot> {
    let mut statuses = tls_ocsp_bundle_specs(config)
        .into_iter()
        .filter_map(|bundle| build_tls_ocsp_status_snapshot(&bundle, runtime_statuses))
        .collect::<Vec<_>>();
    statuses.sort_by(|left, right| left.scope.cmp(&right.scope));
    statuses
}

fn tls_ocsp_bundle_specs(config: &ConfigSnapshot) -> Vec<TlsOcspBundleSpec> {
    let mut bundles = Vec::new();
    for listener in &config.listeners {
        if let Some(tls) = listener.server.tls.as_ref() {
            bundles.push(TlsOcspBundleSpec {
                scope: format!("listener:{}", listener.name),
                cert_path: tls.cert_path.clone(),
                ocsp_staple_path: tls.ocsp_staple_path.clone(),
            });
            bundles.extend(tls.additional_certificates.iter().enumerate().map(
                |(index, bundle)| TlsOcspBundleSpec {
                    scope: format!("listener:{}/additional[{index}]", listener.name),
                    cert_path: bundle.cert_path.clone(),
                    ocsp_staple_path: bundle.ocsp_staple_path.clone(),
                },
            ));
        }
    }

    if let Some(tls) = config.default_vhost.tls.as_ref() {
        bundles.push(TlsOcspBundleSpec {
            scope: format!("vhost:{}", config.default_vhost.id),
            cert_path: tls.cert_path.clone(),
            ocsp_staple_path: tls.ocsp_staple_path.clone(),
        });
        bundles.extend(tls.additional_certificates.iter().enumerate().map(|(index, bundle)| {
            TlsOcspBundleSpec {
                scope: format!("vhost:{}/additional[{index}]", config.default_vhost.id),
                cert_path: bundle.cert_path.clone(),
                ocsp_staple_path: bundle.ocsp_staple_path.clone(),
            }
        }));
    }

    for vhost in &config.vhosts {
        if let Some(tls) = vhost.tls.as_ref() {
            bundles.push(TlsOcspBundleSpec {
                scope: format!("vhost:{}", vhost.id),
                cert_path: tls.cert_path.clone(),
                ocsp_staple_path: tls.ocsp_staple_path.clone(),
            });
            bundles.extend(tls.additional_certificates.iter().enumerate().map(
                |(index, bundle)| TlsOcspBundleSpec {
                    scope: format!("vhost:{}/additional[{index}]", vhost.id),
                    cert_path: bundle.cert_path.clone(),
                    ocsp_staple_path: bundle.ocsp_staple_path.clone(),
                },
            ));
        }
    }

    bundles
}

fn build_tls_ocsp_status_snapshot(
    bundle: &TlsOcspBundleSpec,
    runtime_statuses: Option<&HashMap<String, OcspRuntimeStatusEntry>>,
) -> Option<TlsOcspStatusSnapshot> {
    let (responder_urls, responder_error) =
        match ocsp_responder_urls_for_certificate(&bundle.cert_path) {
            Ok(responder_urls) => (responder_urls, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };
    if bundle.ocsp_staple_path.is_none() && responder_urls.is_empty() && responder_error.is_none() {
        return None;
    }

    let (cache_loaded, cache_size_bytes, cache_modified_unix_ms, cache_error) = bundle
        .ocsp_staple_path
        .as_ref()
        .map(|path| inspect_ocsp_cache_file(&bundle.cert_path, path))
        .unwrap_or((false, None, None, None));
    let runtime = runtime_statuses.and_then(|statuses| statuses.get(&bundle.scope));
    let ocsp_request_result = if bundle.ocsp_staple_path.is_some() && !responder_urls.is_empty() {
        Some(crate::build_ocsp_request_for_certificate(&bundle.cert_path))
    } else {
        None
    };
    let request_error = ocsp_request_result
        .as_ref()
        .and_then(|result| result.as_ref().err().map(|error| error.to_string()));
    let auto_refresh_enabled = bundle.ocsp_staple_path.is_some()
        && !responder_urls.is_empty()
        && responder_error.is_none()
        && request_error.is_none();
    let static_error = cache_error.or(responder_error).or_else(|| {
        if bundle.ocsp_staple_path.is_some() && responder_urls.is_empty() {
            Some("certificate does not expose an OCSP responder URL".to_string())
        } else {
            request_error
        }
    });

    Some(TlsOcspStatusSnapshot {
        scope: bundle.scope.clone(),
        cert_path: bundle.cert_path.clone(),
        ocsp_staple_path: bundle.ocsp_staple_path.clone(),
        responder_urls,
        cache_loaded,
        cache_size_bytes,
        cache_modified_unix_ms,
        auto_refresh_enabled,
        last_refresh_unix_ms: runtime.and_then(|entry| entry.last_refresh_unix_ms),
        refreshes_total: runtime.map(|entry| entry.refreshes_total).unwrap_or(0),
        failures_total: runtime.map(|entry| entry.failures_total).unwrap_or(0),
        last_error: runtime.and_then(|entry| entry.last_error.clone()).or(static_error),
    })
}

fn inspect_ocsp_cache_file(
    cert_path: &std::path::Path,
    path: &PathBuf,
) -> (bool, Option<usize>, Option<u64>, Option<String>) {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (false, None, None, None);
        }
        Err(error) => {
            return (
                false,
                None,
                None,
                Some(format!("failed to stat OCSP cache file `{}`: {error}", path.display())),
            );
        }
    };
    let size = usize::try_from(metadata.len()).ok();
    let modified = metadata.modified().ok().map(unix_time_ms);
    let Some(size_bytes) = size else {
        return (
            false,
            size,
            modified,
            Some("OCSP cache file size exceeds platform limits".to_string()),
        );
    };
    if size_bytes == 0 {
        return (false, Some(0), modified, None);
    }
    if size_bytes > crate::MAX_OCSP_RESPONSE_BYTES {
        return (
            false,
            Some(size_bytes),
            modified,
            Some(format!("OCSP cache file exceeds {} bytes", crate::MAX_OCSP_RESPONSE_BYTES)),
        );
    }

    let cache_error = match std::fs::File::open(path).and_then(|file| {
        use std::io::Read;

        let mut reader = file.take(crate::MAX_OCSP_RESPONSE_BYTES as u64 + 1);
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        Ok(bytes)
    }) {
        Ok(bytes) if bytes.len() > crate::MAX_OCSP_RESPONSE_BYTES => {
            Some(format!("OCSP cache file exceeds {} bytes", crate::MAX_OCSP_RESPONSE_BYTES))
        }
        Ok(bytes) => validate_ocsp_response_for_certificate(cert_path, &bytes)
            .err()
            .map(|error| error.to_string()),
        Err(error) => Some(format!("failed to read OCSP cache file `{}`: {error}", path.display())),
    };
    (cache_error.is_none(), Some(size_bytes), modified, cache_error)
}

#[derive(Debug, Clone)]
struct InspectedCertificate {
    subject: Option<String>,
    issuer: Option<String>,
    serial_number: Option<String>,
    san_dns_names: Vec<String>,
    fingerprint_sha256: Option<String>,
    subject_key_identifier: Option<String>,
    authority_key_identifier: Option<String>,
    is_ca: Option<bool>,
    path_len_constraint: Option<u32>,
    key_usage: Option<String>,
    extended_key_usage: Vec<String>,
    not_before_unix_ms: Option<u64>,
    not_after_unix_ms: Option<u64>,
    expires_in_days: Option<i64>,
    chain_length: usize,
    chain_subjects: Vec<String>,
    chain_diagnostics: Vec<String>,
}

fn inspect_certificate(path: &std::path::Path) -> Option<InspectedCertificate> {
    let certs = load_certificate_chain_der(path).ok()?;
    if certs.is_empty() {
        return None;
    }

    let now_secs = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    let mut chain_subjects = Vec::new();
    let mut chain_entries = Vec::new();
    let mut chain_diagnostics = Vec::new();
    let mut seen_fingerprints = std::collections::HashSet::new();

    for (index, der) in certs.iter().enumerate() {
        let fingerprint_sha256 = fingerprint_sha256(der.as_ref());
        if !seen_fingerprints.insert(fingerprint_sha256.clone()) {
            chain_diagnostics.push(format!(
                "duplicate_certificate_in_chain cert[{index}] sha256={fingerprint_sha256}"
            ));
        }

        match X509Certificate::from_der(der.as_ref()) {
            Ok((_, cert)) => {
                let subject = format!("{}", cert.subject());
                let issuer = format!("{}", cert.issuer());
                let expires_in_days = (cert.validity().not_after.timestamp() - now_secs) / 86_400;
                let basic_constraints = cert.basic_constraints().ok().flatten();
                let key_usage = cert.key_usage().ok().flatten();
                let extended_key_usage = cert.extended_key_usage().ok().flatten();
                let subject_key_identifier = extension_key_identifier(&cert, true);
                let authority_key_identifier = extension_key_identifier(&cert, false);
                if cert.validity().not_after.timestamp() < now_secs {
                    chain_diagnostics.push(format!("cert[{index}] expired"));
                } else if expires_in_days <= TLS_EXPIRY_WARNING_DAYS {
                    chain_diagnostics.push(format!("cert[{index}] expires_in_{expires_in_days}d"));
                }
                if index == 0 && cert.is_ca() {
                    chain_diagnostics.push("leaf_certificate_is_marked_as_ca".to_string());
                }
                if index == 0
                    && key_usage.as_ref().is_some_and(|extension| {
                        !extension.value.digital_signature()
                            && !extension.value.key_encipherment()
                            && !extension.value.key_agreement()
                    })
                {
                    chain_diagnostics
                        .push("leaf_key_usage_may_not_allow_tls_server_auth".to_string());
                }
                if index == 0
                    && extended_key_usage.as_ref().is_some_and(|extension| {
                        !extension.value.any && !extension.value.server_auth
                    })
                {
                    chain_diagnostics.push("leaf_missing_server_auth_eku".to_string());
                }
                if index > 0
                    && !basic_constraints.as_ref().is_some_and(|extension| extension.value.ca)
                {
                    chain_diagnostics
                        .push(format!("cert[{index}] intermediate_or_root_not_marked_as_ca"));
                }
                if index > 0
                    && key_usage.as_ref().is_some_and(|extension| !extension.value.key_cert_sign())
                {
                    chain_diagnostics
                        .push(format!("cert[{index}] intermediate_or_root_missing_key_cert_sign"));
                }
                chain_subjects.push(subject.clone());
                chain_entries.push(InspectedCertificate {
                    subject: Some(subject),
                    issuer: Some(issuer),
                    serial_number: Some(cert.tbs_certificate.raw_serial_as_string()),
                    san_dns_names: cert
                        .subject_alternative_name()
                        .ok()
                        .flatten()
                        .map(|san| {
                            san.value
                                .general_names
                                .iter()
                                .filter_map(|name| match name {
                                    GeneralName::DNSName(dns) => Some(dns.to_string()),
                                    _ => None,
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                    fingerprint_sha256: Some(fingerprint_sha256),
                    subject_key_identifier,
                    authority_key_identifier,
                    is_ca: basic_constraints.as_ref().map(|extension| extension.value.ca),
                    path_len_constraint: basic_constraints
                        .as_ref()
                        .and_then(|extension| extension.value.path_len_constraint),
                    key_usage: key_usage.as_ref().map(|extension| extension.value.to_string()),
                    extended_key_usage: describe_extended_key_usage(extended_key_usage.as_ref()),
                    not_before_unix_ms: cert
                        .validity()
                        .not_before
                        .timestamp()
                        .checked_mul(1000)
                        .and_then(|timestamp| timestamp.try_into().ok()),
                    not_after_unix_ms: cert
                        .validity()
                        .not_after
                        .timestamp()
                        .checked_mul(1000)
                        .and_then(|timestamp| timestamp.try_into().ok()),
                    expires_in_days: Some(expires_in_days),
                    chain_length: certs.len(),
                    chain_subjects: Vec::new(),
                    chain_diagnostics: Vec::new(),
                });
            }
            Err(_) => {
                chain_diagnostics.push(format!("cert[{index}] could_not_be_parsed_as_x509"));
            }
        }
    }

    for index in 0..chain_entries.len().saturating_sub(1) {
        let issuer = chain_entries[index].issuer.as_deref();
        let next_subject = chain_entries[index + 1].subject.as_deref();
        if issuer != next_subject {
            chain_diagnostics.push(format!(
                "chain_link_mismatch cert[{index}]_issuer_to_cert[{}]_subject",
                index + 1
            ));
        }
        if let (Some(aki), Some(ski)) = (
            chain_entries[index].authority_key_identifier.as_deref(),
            chain_entries[index + 1].subject_key_identifier.as_deref(),
        ) && aki != ski
        {
            chain_diagnostics
                .push(format!("chain_aki_ski_mismatch cert[{index}]_to_cert[{}]", index + 1));
        }
        if let Some(path_len_constraint) = chain_entries[index + 1].path_len_constraint {
            let remaining_ca_certs =
                chain_entries[index + 2..].iter().filter(|entry| entry.is_ca == Some(true)).count()
                    as u32;
            if remaining_ca_certs > path_len_constraint {
                chain_diagnostics.push(format!(
                    "cert[{}] path_len_constraint_exceeded remaining_ca_certs={} path_len_constraint={}",
                    index + 1,
                    remaining_ca_certs,
                    path_len_constraint
                ));
            }
        }
    }

    if let Some(leaf) = chain_entries.first() {
        if certs.len() == 1 {
            if leaf.subject != leaf.issuer {
                chain_diagnostics
                    .push("chain_incomplete_single_non_self_signed_certificate".to_string());
            }
        } else if let Some(last) = chain_entries.last()
            && last.subject != last.issuer
        {
            chain_diagnostics.push("chain_incomplete_non_self_signed_top_certificate".to_string());
        }
    }

    let leaf = chain_entries.into_iter().next()?;
    Some(InspectedCertificate {
        chain_length: certs.len(),
        chain_subjects,
        chain_diagnostics,
        ..leaf
    })
}

fn load_certificate_chain_der(path: &std::path::Path) -> std::io::Result<Vec<Vec<u8>>> {
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if !certs.is_empty() {
        return Ok(certs.into_iter().map(|cert| cert.as_ref().to_vec()).collect());
    }
    Ok(vec![std::fs::read(path)?])
}

fn fingerprint_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>()
}

fn extension_key_identifier(cert: &X509Certificate<'_>, subject: bool) -> Option<String> {
    cert.iter_extensions().find_map(|extension| match extension.parsed_extension() {
        ParsedExtension::SubjectKeyIdentifier(identifier) if subject => {
            Some(format!("{identifier:x}"))
        }
        ParsedExtension::AuthorityKeyIdentifier(identifier) if !subject => {
            identifier.key_identifier.as_ref().map(|identifier| format!("{identifier:x}"))
        }
        _ => None,
    })
}

fn describe_extended_key_usage(
    extension: Option<
        &x509_parser::certificate::BasicExtension<&x509_parser::extensions::ExtendedKeyUsage<'_>>,
    >,
) -> Vec<String> {
    let Some(extension) = extension else {
        return Vec::new();
    };

    let mut usages = Vec::new();
    if extension.value.any {
        usages.push("any".to_string());
    }
    if extension.value.server_auth {
        usages.push("server_auth".to_string());
    }
    if extension.value.client_auth {
        usages.push("client_auth".to_string());
    }
    if extension.value.code_signing {
        usages.push("code_signing".to_string());
    }
    if extension.value.email_protection {
        usages.push("email_protection".to_string());
    }
    if extension.value.time_stamping {
        usages.push("time_stamping".to_string());
    }
    if extension.value.ocsp_signing {
        usages.push("ocsp_signing".to_string());
    }
    usages.extend(extension.value.other.iter().map(|oid| oid.to_id_string()));
    usages
}

fn tls_version_label(version: rginx_core::TlsVersion) -> &'static str {
    match version {
        rginx_core::TlsVersion::Tls12 => "TLS1.2",
        rginx_core::TlsVersion::Tls13 => "TLS1.3",
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
        BasicConstraints, CertificateParams, CertifiedKey, DnType, ExtendedKeyUsagePurpose, IsCa,
        KeyPair,
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
        let ca_cert = ca_params.self_signed(&ca_key).expect("CA should self-sign");
        let ca = CertifiedKey { cert: ca_cert, key_pair: ca_key };

        let mut leaf_params =
            CertificateParams::new(vec!["leaf.example.com".to_string()]).expect("leaf params");
        leaf_params.distinguished_name.push(DnType::CommonName, "leaf.example.com");
        leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let leaf_key = KeyPair::generate().expect("leaf key should generate");
        let leaf_cert = leaf_params
            .signed_by(&leaf_key, &ca.cert, &ca.key_pair)
            .expect("leaf should be signed by CA");

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
        let ca_cert = ca_params.self_signed(&ca_key).expect("CA should self-sign");
        let ca = CertifiedKey { cert: ca_cert, key_pair: ca_key };

        let mut leaf_params = CertificateParams::new(vec!["client-only.example.com".to_string()])
            .expect("leaf params");
        leaf_params.distinguished_name.push(DnType::CommonName, "client-only.example.com");
        leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let leaf_key = KeyPair::generate().expect("leaf key should generate");
        let leaf_cert = leaf_params
            .signed_by(&leaf_key, &ca.cert, &ca.key_pair)
            .expect("leaf should be signed by CA");

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
