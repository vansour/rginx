use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use http::StatusCode;

#[derive(Debug, Clone)]
pub struct ConfigSnapshot {
    pub runtime: RuntimeSettings,
    pub server: Server,
    pub routes: Vec<Route>,
    pub upstreams: HashMap<String, Arc<Upstream>>,
}

#[derive(Debug, Clone)]
pub struct RuntimeSettings {
    pub shutdown_timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct Server {
    pub listen_addr: SocketAddr,
}

#[derive(Debug, Clone)]
pub struct Route {
    pub matcher: RouteMatcher,
    pub action: RouteAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteMatcher {
    Exact(String),
    Prefix(String),
}

impl RouteMatcher {
    pub fn matches(&self, path: &str) -> bool {
        match self {
            Self::Exact(expected) => path == expected,
            Self::Prefix(prefix) if prefix == "/" => true,
            Self::Prefix(prefix) => {
                path == prefix
                    || path.strip_prefix(prefix).is_some_and(|remainder| remainder.starts_with('/'))
            }
        }
    }

    pub fn priority(&self) -> (u8, usize) {
        match self {
            Self::Exact(path) => (2, path.len()),
            Self::Prefix(path) => (1, path.len()),
        }
    }
}

#[derive(Debug, Clone)]
pub enum RouteAction {
    Static(StaticResponse),
    Proxy(ProxyTarget),
}

#[derive(Debug, Clone)]
pub struct StaticResponse {
    pub status: StatusCode,
    pub content_type: String,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct ProxyTarget {
    pub upstream_name: String,
    pub upstream: Arc<Upstream>,
}

#[derive(Debug)]
pub struct Upstream {
    pub name: String,
    pub peers: Vec<UpstreamPeer>,
    pub tls: UpstreamTls,
    pub server_name_override: Option<String>,
    cursor: AtomicUsize,
}

impl Upstream {
    pub fn new(
        name: String,
        peers: Vec<UpstreamPeer>,
        tls: UpstreamTls,
        server_name_override: Option<String>,
    ) -> Self {
        Self { name, peers, tls, server_name_override, cursor: AtomicUsize::new(0) }
    }

    pub fn next_peer(&self) -> Option<UpstreamPeer> {
        if self.peers.is_empty() {
            return None;
        }

        let index = self.cursor.fetch_add(1, Ordering::Relaxed) % self.peers.len();
        Some(self.peers[index].clone())
    }
}

#[derive(Debug, Clone)]
pub struct UpstreamPeer {
    pub url: String,
    pub scheme: String,
    pub authority: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UpstreamTls {
    NativeRoots,
    CustomCa { ca_cert_path: PathBuf },
    Insecure,
}
