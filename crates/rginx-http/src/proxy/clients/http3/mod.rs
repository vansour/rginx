//! Upstream HTTP/3 session reuse, endpoint caching, and body bridging.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use super::*;
use endpoint_cache::Http3ClientEndpoints;
use session::{Http3SessionEntry, Http3SessionKey};

#[cfg(test)]
use crate::handler::boxed_body;
#[cfg(test)]
use std::net::SocketAddr;
#[cfg(test)]
use tokio::task::JoinHandle;

mod connect;
mod endpoint_cache;
mod request;
mod response_body;
mod session;
#[cfg(test)]
mod tests;

#[derive(Clone)]
pub(crate) struct Http3Client {
    client_config: quinn::ClientConfig,
    connect_timeout: Duration,
    resolver: Arc<UpstreamResolver>,
    endpoints: Arc<Http3ClientEndpoints>,
    sessions: Arc<Mutex<HashMap<Http3SessionKey, Http3SessionEntry>>>,
}

impl Http3Client {
    pub(super) fn new(
        client_config: quinn::ClientConfig,
        connect_timeout: Duration,
        resolver: Arc<UpstreamResolver>,
    ) -> Self {
        Self {
            client_config,
            connect_timeout,
            resolver,
            endpoints: Arc::new(Http3ClientEndpoints::default()),
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(super) async fn resolve_peer(
        &self,
        peer: &UpstreamPeer,
    ) -> Result<Vec<ResolvedUpstreamPeer>, Error> {
        self.resolver.resolve_peer(peer).await
    }

    pub(super) async fn cached_peer_endpoints(
        &self,
        peer: &UpstreamPeer,
    ) -> Result<Vec<ResolvedUpstreamPeer>, Error> {
        self.resolver.cached_peer_endpoints(peer).await
    }

    pub(super) async fn resolver_snapshot(&self) -> UpstreamResolverRuntimeSnapshot {
        self.resolver.snapshot().await
    }
}
