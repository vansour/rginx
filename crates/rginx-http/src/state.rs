use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rginx_core::{ConfigSnapshot, Result};
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

#[derive(Clone)]
pub struct SharedState {
    inner: Arc<RwLock<ActiveState>>,
    revisions: watch::Sender<u64>,
    metrics: Metrics,
    rate_limiters: RateLimiters,
    tls_acceptor: Arc<RwLock<Option<TlsAcceptor>>>,
    background_tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
    request_ids: Arc<AtomicU64>,
}

impl SharedState {
    pub fn from_config(config: ConfigSnapshot) -> Result<Self> {
        let config = Arc::new(config);
        let clients = ProxyClients::from_config(config.as_ref())?;
        let tls_acceptor = build_tls_acceptor(&config.default_vhost, &config.vhosts)?;
        let revision = 0u64;
        let (revisions, _rx) = watch::channel(revision);
        let metrics = Metrics::default();
        let rate_limiters = RateLimiters::default();

        Ok(Self {
            inner: Arc::new(RwLock::new(ActiveState { revision, config, clients })),
            revisions,
            metrics,
            rate_limiters,
            tls_acceptor: Arc::new(RwLock::new(tls_acceptor)),
            background_tasks: Arc::new(Mutex::new(Vec::new())),
            request_ids: Arc::new(AtomicU64::new(1)),
        })
    }

    pub async fn snapshot(&self) -> ActiveState {
        self.inner.read().await.clone()
    }

    pub async fn current_config(&self) -> Arc<ConfigSnapshot> {
        self.inner.read().await.config.clone()
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
        let config = Arc::new(config);
        let clients = ProxyClients::from_config(config.as_ref())?;
        let tls_acceptor = build_tls_acceptor(&config.default_vhost, &config.vhosts)?;
        let next_revision = *self.revisions.borrow() + 1;

        {
            let mut state = self.inner.write().await;
            state.revision = next_revision;
            state.config = config.clone();
            state.clients = clients;
        }
        *self.tls_acceptor.write().await = tls_acceptor;
        let _ = self.revisions.send(next_revision);
        Ok(config)
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

fn take_background_tasks(
    background_tasks: &Arc<Mutex<Vec<JoinHandle<()>>>>,
) -> Vec<JoinHandle<()>> {
    let mut tasks = background_tasks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    std::mem::take(&mut *tasks)
}
