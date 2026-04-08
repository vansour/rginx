use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock as StdRwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use http::StatusCode;
use rginx_core::{ConfigSnapshot, Error, Listener, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, RwLock, watch};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

use crate::proxy::{HealthChangeNotifier, ProxyClients, UpstreamHealthSnapshot};
use crate::rate_limit::RateLimiters;
use crate::tls::build_tls_acceptor;

const RECENT_WINDOW_SECS: u64 = 60;
const MAX_RECENT_WINDOW_SECS: u64 = 300;

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
                    upstream_name,
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

    pub fn record_reload_success(&self, revision: u64) {
        let mut history =
            self.reload_history.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        history.attempts_total += 1;
        history.successes_total += 1;
        history.last_result = Some(ReloadResultSnapshot {
            finished_at_unix_ms: unix_time_ms(SystemTime::now()),
            outcome: ReloadOutcomeSnapshot::Success { revision },
        });
        self.mark_snapshot_changed_components(true, false, false, false, false);
    }

    pub fn record_reload_failure(&self, error: impl Into<String>) {
        let mut history =
            self.reload_history.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        history.attempts_total += 1;
        history.failures_total += 1;
        history.last_result = Some(ReloadResultSnapshot {
            finished_at_unix_ms: unix_time_ms(SystemTime::now()),
            outcome: ReloadOutcomeSnapshot::Failure { error: error.into() },
        });
        self.mark_snapshot_changed_components(true, false, false, false, false);
    }

    pub async fn tls_acceptor(&self, listener_id: &str) -> Option<TlsAcceptor> {
        self.listener_tls_acceptors.read().await.get(listener_id).cloned().flatten()
    }

    pub async fn replace(&self, config: ConfigSnapshot) -> Result<Arc<ConfigSnapshot>> {
        let prepared = self.prepare_replacement(config).await?;
        Ok(self.commit_prepared(prepared).await)
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use http::StatusCode;
    use rginx_core::{
        Listener, ReturnAction, Route, RouteAccessControl, RouteAction, RouteMatcher,
        RuntimeSettings, Server, Upstream, UpstreamLoadBalance, UpstreamPeer, UpstreamProtocol,
        UpstreamSettings, UpstreamTls, VirtualHost,
    };

    use super::{ConfigSnapshot, ReloadOutcomeSnapshot, SharedState, validate_config_transition};

    fn snapshot(listen: &str) -> ConfigSnapshot {
        let server = Server {
            listen_addr: listen.parse().unwrap(),
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
    }

    #[test]
    fn reload_status_snapshot_tracks_last_success_and_failure() {
        let shared = SharedState::from_config(snapshot("127.0.0.1:8080"))
            .expect("shared state should build");

        shared.record_reload_success(2);
        let first = shared.reload_status_snapshot();
        assert_eq!(first.attempts_total, 1);
        assert_eq!(first.successes_total, 1);
        assert_eq!(first.failures_total, 0);
        assert!(matches!(
            first.last_result.as_ref().map(|result| &result.outcome),
            Some(ReloadOutcomeSnapshot::Success { revision: 2 })
        ));

        shared.record_reload_failure("bad config");
        let second = shared.reload_status_snapshot();
        assert_eq!(second.attempts_total, 2);
        assert_eq!(second.successes_total, 1);
        assert_eq!(second.failures_total, 1);
        assert!(matches!(
            second.last_result.as_ref().map(|result| &result.outcome),
            Some(ReloadOutcomeSnapshot::Failure { error }) if error == "bad config"
        ));
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
