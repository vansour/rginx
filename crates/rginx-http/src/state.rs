use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use http::StatusCode;
use rginx_core::{ConfigSnapshot, Error, Listener, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, watch};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

use crate::proxy::{ProxyClients, UpstreamHealthSnapshot};
use crate::rate_limit::RateLimiters;
use crate::tls::build_tls_acceptor;

#[derive(Clone)]
pub struct ActiveState {
    pub revision: u64,
    pub config: Arc<ConfigSnapshot>,
    pub clients: ProxyClients,
}

struct PreparedState {
    config: Arc<ConfigSnapshot>,
    clients: ProxyClients,
    listener_tls_acceptors: HashMap<String, Option<TlsAcceptor>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpCountersSnapshot {
    pub downstream_connections_accepted: u64,
    pub downstream_connections_rejected: u64,
    pub downstream_requests: u64,
    pub downstream_responses: u64,
    pub downstream_responses_1xx: u64,
    pub downstream_responses_2xx: u64,
    pub downstream_responses_3xx: u64,
    pub downstream_responses_4xx: u64,
    pub downstream_responses_5xx: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReloadOutcomeSnapshot {
    Success { revision: u64 },
    Failure { error: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReloadResultSnapshot {
    pub finished_at_unix_ms: u64,
    pub outcome: ReloadOutcomeSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReloadStatusSnapshot {
    pub attempts_total: u64,
    pub successes_total: u64,
    pub failures_total: u64,
    pub last_result: Option<ReloadResultSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeStatusSnapshot {
    pub revision: u64,
    pub config_path: Option<PathBuf>,
    pub listen_addr: std::net::SocketAddr,
    pub worker_threads: Option<usize>,
    pub accept_workers: usize,
    pub total_vhosts: usize,
    pub total_routes: usize,
    pub total_upstreams: usize,
    pub tls_enabled: bool,
    pub active_connections: usize,
    pub reload: ReloadStatusSnapshot,
}

#[derive(Debug, Default)]
struct HttpCounters {
    downstream_connections_accepted: AtomicU64,
    downstream_connections_rejected: AtomicU64,
    downstream_requests: AtomicU64,
    downstream_responses: AtomicU64,
    downstream_responses_1xx: AtomicU64,
    downstream_responses_2xx: AtomicU64,
    downstream_responses_3xx: AtomicU64,
    downstream_responses_4xx: AtomicU64,
    downstream_responses_5xx: AtomicU64,
}

#[derive(Debug, Default)]
struct ReloadHistory {
    attempts_total: u64,
    successes_total: u64,
    failures_total: u64,
    last_result: Option<ReloadResultSnapshot>,
}

#[derive(Clone)]
pub struct SharedState {
    inner: Arc<RwLock<ActiveState>>,
    revisions: watch::Sender<u64>,
    rate_limiters: RateLimiters,
    listener_tls_acceptors: Arc<RwLock<HashMap<String, Option<TlsAcceptor>>>>,
    listener_active_connections: Arc<HashMap<String, Arc<AtomicUsize>>>,
    background_tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
    active_connections: Arc<AtomicUsize>,
    counters: Arc<HttpCounters>,
    reload_history: Arc<Mutex<ReloadHistory>>,
    request_ids: Arc<AtomicU64>,
    config_path: Option<Arc<PathBuf>>,
}

pub struct ActiveConnectionGuard {
    active_connections: Arc<AtomicUsize>,
    listener_active_connections: Arc<AtomicUsize>,
}

impl Drop for ActiveConnectionGuard {
    fn drop(&mut self) {
        self.active_connections.fetch_sub(1, Ordering::AcqRel);
        self.listener_active_connections.fetch_sub(1, Ordering::AcqRel);
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
        let prepared = prepare_state(config)?;
        let revision = 0u64;
        let (revisions, _rx) = watch::channel(revision);
        let rate_limiters = RateLimiters::default();
        let listener_active_connections = prepared
            .config
            .listeners
            .iter()
            .map(|listener| (listener.id.clone(), Arc::new(AtomicUsize::new(0))))
            .collect::<HashMap<_, _>>();

        Ok(Self {
            inner: Arc::new(RwLock::new(ActiveState {
                revision,
                config: prepared.config,
                clients: prepared.clients,
            })),
            revisions,
            rate_limiters,
            listener_tls_acceptors: Arc::new(RwLock::new(prepared.listener_tls_acceptors)),
            listener_active_connections: Arc::new(listener_active_connections),
            background_tasks: Arc::new(Mutex::new(Vec::new())),
            active_connections: Arc::new(AtomicUsize::new(0)),
            counters: Arc::new(HttpCounters::default()),
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
        ActiveConnectionGuard {
            active_connections: self.active_connections.clone(),
            listener_active_connections,
        }
    }

    pub(crate) fn record_connection_accepted(&self) {
        self.counters.downstream_connections_accepted.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_connection_rejected(&self) {
        self.counters.downstream_connections_rejected.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_downstream_request(&self) {
        self.counters.downstream_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_downstream_response(&self, status: StatusCode) {
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
        prepare_state(config)
    }

    async fn commit_prepared(&self, prepared: PreparedState) -> Arc<ConfigSnapshot> {
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
}

fn prepare_state(config: ConfigSnapshot) -> Result<PreparedState> {
    let config = Arc::new(config);
    let clients = ProxyClients::from_config(config.as_ref())?;
    let listener_tls_acceptors = config
        .listeners
        .iter()
        .map(|listener| {
            let tls_acceptor = build_tls_acceptor(
                listener.server.tls.as_ref(),
                listener.tls_enabled(),
                &config.default_vhost,
                &config.vhosts,
            )?;
            Ok((listener.id.clone(), tls_acceptor))
        })
        .collect::<Result<HashMap<_, _>>>()?;

    Ok(PreparedState { config, clients, listener_tls_acceptors })
}

impl HttpCounters {
    fn snapshot(&self) -> HttpCountersSnapshot {
        HttpCountersSnapshot {
            downstream_connections_accepted: self
                .downstream_connections_accepted
                .load(Ordering::Relaxed),
            downstream_connections_rejected: self
                .downstream_connections_rejected
                .load(Ordering::Relaxed),
            downstream_requests: self.downstream_requests.load(Ordering::Relaxed),
            downstream_responses: self.downstream_responses.load(Ordering::Relaxed),
            downstream_responses_1xx: self.downstream_responses_1xx.load(Ordering::Relaxed),
            downstream_responses_2xx: self.downstream_responses_2xx.load(Ordering::Relaxed),
            downstream_responses_3xx: self.downstream_responses_3xx.load(Ordering::Relaxed),
            downstream_responses_4xx: self.downstream_responses_4xx.load(Ordering::Relaxed),
            downstream_responses_5xx: self.downstream_responses_5xx.load(Ordering::Relaxed),
        }
    }
}

impl ReloadHistory {
    fn snapshot(&self) -> ReloadStatusSnapshot {
        ReloadStatusSnapshot {
            attempts_total: self.attempts_total,
            successes_total: self.successes_total,
            failures_total: self.failures_total,
            last_result: self.last_result.clone(),
        }
    }
}

fn unix_time_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

pub fn validate_config_transition(current: &ConfigSnapshot, next: &ConfigSnapshot) -> Result<()> {
    if current.listeners.len() != next.listeners.len() {
        return Err(Error::Config(format!(
            "reloading listener count from `{}` to `{}` is not supported; restart rginx instead",
            current.listeners.len(),
            next.listeners.len()
        )));
    }

    for (current_listener, next_listener) in current.listeners.iter().zip(next.listeners.iter()) {
        if current_listener.id != next_listener.id {
            return Err(Error::Config(format!(
                "reloading listener id from `{}` to `{}` is not supported; restart rginx instead",
                current_listener.id, next_listener.id
            )));
        }

        if current_listener.server.listen_addr != next_listener.server.listen_addr {
            return Err(Error::Config(format!(
                "reloading listen address for listener `{}` from `{}` to `{}` is not supported; restart rginx instead",
                current_listener.id,
                current_listener.server.listen_addr,
                next_listener.server.listen_addr
            )));
        }
    }

    if current.runtime.worker_threads != next.runtime.worker_threads {
        return Err(Error::Config(format!(
            "reloading runtime.worker_threads from `{:?}` to `{:?}` is not supported; restart rginx instead",
            current.runtime.worker_threads, next.runtime.worker_threads
        )));
    }

    if current.runtime.accept_workers != next.runtime.accept_workers {
        return Err(Error::Config(format!(
            "reloading runtime.accept_workers from `{}` to `{}` is not supported; restart rginx instead",
            current.runtime.accept_workers, next.runtime.accept_workers
        )));
    }

    Ok(())
}

fn take_background_tasks(
    background_tasks: &Arc<Mutex<Vec<JoinHandle<()>>>>,
) -> Vec<JoinHandle<()>> {
    let mut tasks = background_tasks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    std::mem::take(&mut *tasks)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::Duration;

    use http::StatusCode;
    use rginx_core::{Listener, RuntimeSettings, Server, VirtualHost};

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
        assert!(error.to_string().contains("restart rginx"));
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

        shared.record_connection_accepted();
        shared.record_connection_rejected();
        shared.record_downstream_request();
        shared.record_downstream_request();
        shared.record_downstream_response(StatusCode::OK);
        shared.record_downstream_response(StatusCode::NOT_FOUND);
        shared.record_downstream_response(StatusCode::BAD_GATEWAY);

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
}
