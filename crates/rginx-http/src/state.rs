use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use rginx_core::{ConfigSnapshot, Error, Result};
use tokio::sync::{RwLock, watch};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

use crate::metrics::Metrics;
use crate::proxy::ProxyClients;
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
    tls_acceptor: Option<TlsAcceptor>,
}

#[derive(Clone)]
pub struct SharedState {
    inner: Arc<RwLock<ActiveState>>,
    revisions: watch::Sender<u64>,
    metrics: Metrics,
    rate_limiters: RateLimiters,
    tls_acceptor: Arc<RwLock<Option<TlsAcceptor>>>,
    background_tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
    request_ids: Arc<AtomicU64>,
    config_path: Option<Arc<PathBuf>>,
    config_source: Arc<RwLock<Option<String>>>,
}

impl SharedState {
    pub fn from_config(config: ConfigSnapshot) -> Result<Self> {
        Self::from_parts(config, None, None)
    }

    pub fn from_config_path(config_path: PathBuf, config: ConfigSnapshot) -> Result<Self> {
        let config_source = fs::read_to_string(&config_path)?;
        Self::from_parts(config, Some(config_path), Some(config_source))
    }

    fn from_parts(
        config: ConfigSnapshot,
        config_path: Option<PathBuf>,
        config_source: Option<String>,
    ) -> Result<Self> {
        let prepared = prepare_state(config)?;
        let revision = 0u64;
        let (revisions, _rx) = watch::channel(revision);
        let metrics = Metrics::default();
        let rate_limiters = RateLimiters::default();

        Ok(Self {
            inner: Arc::new(RwLock::new(ActiveState {
                revision,
                config: prepared.config,
                clients: prepared.clients,
            })),
            revisions,
            metrics,
            rate_limiters,
            tls_acceptor: Arc::new(RwLock::new(prepared.tls_acceptor)),
            background_tasks: Arc::new(Mutex::new(Vec::new())),
            request_ids: Arc::new(AtomicU64::new(1)),
            config_path: config_path.map(Arc::new),
            config_source: Arc::new(RwLock::new(config_source)),
        })
    }

    pub async fn snapshot(&self) -> ActiveState {
        self.inner.read().await.clone()
    }

    pub async fn current_config(&self) -> Arc<ConfigSnapshot> {
        self.inner.read().await.config.clone()
    }

    pub async fn active_config_source(&self) -> Option<String> {
        self.config_source.read().await.clone()
    }

    pub fn persistent_config_path(&self) -> Option<PathBuf> {
        self.config_path.as_deref().cloned()
    }

    pub fn subscribe_updates(&self) -> watch::Receiver<u64> {
        self.revisions.subscribe()
    }

    pub fn metrics(&self) -> Metrics {
        self.metrics.clone()
    }

    pub fn rate_limiters(&self) -> RateLimiters {
        self.rate_limiters.clone()
    }

    pub async fn tls_acceptor(&self) -> Option<TlsAcceptor> {
        self.tls_acceptor.read().await.clone()
    }

    pub async fn replace(&self, config: ConfigSnapshot) -> Result<Arc<ConfigSnapshot>> {
        let prepared = self.prepare_replacement(config).await?;
        Ok(self.commit_prepared(prepared, None).await)
    }

    pub async fn replace_with_source(
        &self,
        config: ConfigSnapshot,
        config_source: String,
    ) -> Result<Arc<ConfigSnapshot>> {
        let prepared = self.prepare_replacement(config).await?;
        Ok(self.commit_prepared(prepared, Some(config_source)).await)
    }

    pub async fn apply_config_source(&self, config_source: String) -> Result<Arc<ConfigSnapshot>> {
        let config_path = self.persistent_config_path().ok_or_else(|| {
            Error::Server(
                "dynamic config API is unavailable without a runtime-backed config path"
                    .to_string(),
            )
        })?;
        let next = rginx_config::load_and_compile_from_str(&config_source, &config_path)?;
        let prepared = self.prepare_replacement(next).await?;
        write_config_atomically(&config_path, &config_source).await?;
        Ok(self.commit_prepared(prepared, Some(config_source)).await)
    }

    async fn prepare_replacement(&self, config: ConfigSnapshot) -> Result<PreparedState> {
        let current = self.current_config().await;
        validate_config_transition(current.as_ref(), &config)?;
        prepare_state(config)
    }

    async fn commit_prepared(
        &self,
        prepared: PreparedState,
        config_source: Option<String>,
    ) -> Arc<ConfigSnapshot> {
        let next_revision = {
            let mut state = self.inner.write().await;
            let next_revision = state.revision + 1;
            state.revision = next_revision;
            state.config = prepared.config.clone();
            state.clients = prepared.clients;
            next_revision
        };

        *self.tls_acceptor.write().await = prepared.tls_acceptor;
        if let Some(config_source) = config_source {
            *self.config_source.write().await = Some(config_source);
        }
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
    let tls_acceptor = build_tls_acceptor(&config.default_vhost, &config.vhosts)?;

    Ok(PreparedState { config, clients, tls_acceptor })
}

pub fn validate_config_transition(current: &ConfigSnapshot, next: &ConfigSnapshot) -> Result<()> {
    if current.server.listen_addr != next.server.listen_addr {
        return Err(Error::Config(format!(
            "reloading listen address from `{}` to `{}` is not supported; restart rginx instead",
            current.server.listen_addr, next.server.listen_addr
        )));
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

async fn write_config_atomically(path: &Path, config_source: &str) -> Result<()> {
    let temp_path = temp_config_path(path);
    tokio::fs::write(&temp_path, config_source).await?;

    match tokio::fs::rename(&temp_path, path).await {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = tokio::fs::remove_file(&temp_path).await;
            Err(error.into())
        }
    }
}

fn temp_config_path(path: &Path) -> PathBuf {
    let file_name = path.file_name().and_then(|value| value.to_str()).unwrap_or("rginx.ron");
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let temp_name = format!(".{file_name}.tmp-{suffix}");

    path.parent().unwrap_or_else(|| Path::new(".")).join(temp_name)
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
    use std::time::Duration;

    use rginx_core::{RuntimeSettings, Server, VirtualHost};

    use super::{ConfigSnapshot, validate_config_transition};

    fn snapshot(listen: &str) -> ConfigSnapshot {
        ConfigSnapshot {
            runtime: RuntimeSettings {
                shutdown_timeout: Duration::from_secs(10),
                worker_threads: None,
                accept_workers: 1,
            },
            server: Server {
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
                config_api_token: None,
                tls: None,
            },
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
}
