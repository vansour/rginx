use std::sync::Arc;

use rginx_core::{ConfigSnapshot, Result};
use tokio::sync::{watch, RwLock};
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
}

impl SharedState {
    pub fn from_config(config: ConfigSnapshot) -> Result<Self> {
        let config = Arc::new(config);
        let clients = ProxyClients::from_config(config.as_ref())?;
        let tls_acceptor = build_tls_acceptor(&config.server)?;
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
        let tls_acceptor = build_tls_acceptor(&config.server)?;
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
}
