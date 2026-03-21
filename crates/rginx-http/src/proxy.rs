use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::future::Future;
use std::io::BufReader;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::Instant;

use bytes::{Bytes, BytesMut};
use http::header::{
    HeaderMap, HeaderName, HeaderValue, CONNECTION, HOST, PROXY_AUTHENTICATE, PROXY_AUTHORIZATION,
    TE, TRAILER, TRANSFER_ENCODING, UPGRADE,
};
use http::{Method, Request, Response, StatusCode, Uri, Version};
use http_body_util::BodyExt;
use hyper::body::Body as _;
use hyper::body::Incoming;
use hyper_rustls::{
    ConfigBuilderExt, FixedServerNameResolver, HttpsConnector, HttpsConnectorBuilder,
};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use rginx_core::{ConfigSnapshot, Error, ProxyTarget, Upstream, UpstreamPeer, UpstreamTls};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use serde::Serialize;

use crate::client_ip::ClientAddress;
use crate::handler::{full_body, BoxError, HttpBody, HttpResponse};
use crate::metrics::Metrics;
use crate::timeout::IdleTimeoutBody;

const MAX_FAILOVER_ATTEMPTS: usize = 2;

pub type ProxyClient = Client<HttpsConnector<HttpConnector>, HttpBody>;

struct PreparedProxyRequest {
    method: Method,
    version: Version,
    uri: Uri,
    headers: HeaderMap,
    body: PreparedRequestBody,
}

enum PreparedRequestBody {
    Replayable(Bytes),
    Streaming(Option<Incoming>),
}

#[derive(Debug)]
enum PrepareRequestError {
    PayloadTooLarge { max_request_body_bytes: usize },
    Other(Box<dyn std::error::Error + Send + Sync>),
}

impl PrepareRequestError {
    fn payload_too_large(max_request_body_bytes: usize) -> Self {
        Self::PayloadTooLarge { max_request_body_bytes }
    }

    fn other(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Other(Box::new(error))
    }
}

impl std::fmt::Display for PrepareRequestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PayloadTooLarge { max_request_body_bytes } => write!(
                formatter,
                "request body exceeded configured limit of {max_request_body_bytes} bytes"
            ),
            Self::Other(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for PrepareRequestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::PayloadTooLarge { .. } => None,
            Self::Other(error) => Some(error.as_ref()),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PeerHealthPolicy {
    unhealthy_after_failures: u32,
    cooldown: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PeerHealthKey {
    upstream_name: String,
    peer_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PeerFailureStatus {
    consecutive_failures: u32,
    entered_cooldown: bool,
}

#[derive(Debug, Default)]
struct PassiveHealthState {
    consecutive_failures: u32,
    unhealthy_until: Option<Instant>,
}

#[derive(Debug, Default)]
struct ActiveHealthState {
    unhealthy: bool,
    consecutive_successes: u32,
}

#[derive(Debug, Default)]
struct PeerHealthState {
    passive: PassiveHealthState,
    active: ActiveHealthState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveProbeStatus {
    healthy: bool,
    recovered: bool,
    consecutive_successes: u32,
}

#[derive(Debug, Default)]
struct PeerHealth {
    state: Mutex<PeerHealthState>,
}

#[derive(Clone)]
struct PeerHealthRegistry {
    policies: Arc<HashMap<String, PeerHealthPolicy>>,
    peers: Arc<HashMap<PeerHealthKey, Arc<PeerHealth>>>,
}

struct SelectedPeers {
    peers: Vec<UpstreamPeer>,
    skipped_unhealthy: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PeerStatusSnapshot {
    pub url: String,
    pub healthy: bool,
    pub passive_consecutive_failures: u32,
    pub passive_cooldown_remaining_ms: Option<u64>,
    pub active_unhealthy: bool,
    pub active_consecutive_successes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TlsClientProfile {
    tls: UpstreamTls,
    server_name_override: Option<String>,
}

impl TlsClientProfile {
    fn from_upstream(upstream: &Upstream) -> Self {
        Self {
            tls: upstream.tls.clone(),
            server_name_override: upstream.server_name_override.clone(),
        }
    }
}

impl PeerHealthPolicy {
    fn from_upstream(upstream: &Upstream) -> Self {
        Self {
            unhealthy_after_failures: upstream.unhealthy_after_failures,
            cooldown: upstream.unhealthy_cooldown,
        }
    }
}

#[derive(Clone)]
pub struct ProxyClients {
    clients: Arc<HashMap<TlsClientProfile, ProxyClient>>,
    health: PeerHealthRegistry,
}

impl ProxyClients {
    pub fn from_config(config: &ConfigSnapshot) -> Result<Self, Error> {
        let profiles = config
            .upstreams
            .values()
            .map(|upstream| TlsClientProfile::from_upstream(upstream.as_ref()))
            .collect::<HashSet<_>>();

        let mut clients = HashMap::new();
        for profile in profiles {
            let client = build_client_for_profile(&profile)?;
            clients.insert(profile, client);
        }

        Ok(Self { clients: Arc::new(clients), health: PeerHealthRegistry::from_config(config) })
    }

    pub fn for_upstream(&self, upstream: &Upstream) -> Result<ProxyClient, Error> {
        let profile = TlsClientProfile::from_upstream(upstream);
        self.clients.get(&profile).cloned().ok_or_else(|| {
            Error::Server(format!(
                "missing cached proxy client for upstream `{}` with TLS profile {:?}",
                upstream.name, profile
            ))
        })
    }

    fn select_peers(&self, upstream: &Upstream, limit: usize) -> SelectedPeers {
        self.health.select_peers(upstream, limit)
    }

    fn record_peer_success(&self, upstream_name: &str, peer_url: &str) {
        self.health.record_success(upstream_name, peer_url);
    }

    fn record_peer_failure(&self, upstream_name: &str, peer_url: &str) -> PeerFailureStatus {
        self.health.record_failure(upstream_name, peer_url)
    }

    fn record_active_peer_success(
        &self,
        upstream_name: &str,
        peer_url: &str,
        healthy_successes_required: u32,
    ) -> ActiveProbeStatus {
        self.health.record_active_success(upstream_name, peer_url, healthy_successes_required)
    }

    pub(crate) fn record_active_peer_failure(&self, upstream_name: &str, peer_url: &str) -> bool {
        self.health.record_active_failure(upstream_name, peer_url)
    }

    pub(crate) fn peer_statuses(&self, upstream: &Upstream) -> Vec<PeerStatusSnapshot> {
        upstream
            .peers
            .iter()
            .map(|peer| self.health.snapshot(&upstream.name, &peer.url, &peer.url))
            .collect()
    }
}

impl PeerHealthRegistry {
    fn from_config(config: &ConfigSnapshot) -> Self {
        let policies = config
            .upstreams
            .iter()
            .map(|(upstream_name, upstream)| {
                (upstream_name.clone(), PeerHealthPolicy::from_upstream(upstream.as_ref()))
            })
            .collect::<HashMap<_, _>>();
        let peers = config
            .upstreams
            .iter()
            .flat_map(|(upstream_name, upstream)| {
                upstream.peers.iter().map(|peer| {
                    (
                        PeerHealthKey {
                            upstream_name: upstream_name.clone(),
                            peer_url: peer.url.clone(),
                        },
                        Arc::new(PeerHealth::default()),
                    )
                })
            })
            .collect::<HashMap<_, _>>();

        Self { policies: Arc::new(policies), peers: Arc::new(peers) }
    }

    fn select_peers(&self, upstream: &Upstream, limit: usize) -> SelectedPeers {
        if limit == 0 {
            return SelectedPeers { peers: Vec::new(), skipped_unhealthy: 0 };
        }

        let mut selected = Vec::new();
        let mut skipped_unhealthy = 0;

        for peer in upstream.next_peers(upstream.peers.len()) {
            if self.is_available(&upstream.name, &peer.url) {
                selected.push(peer);
                if selected.len() == limit {
                    break;
                }
            } else {
                skipped_unhealthy += 1;
            }
        }

        SelectedPeers { peers: selected, skipped_unhealthy }
    }

    fn record_success(&self, upstream_name: &str, peer_url: &str) {
        if let Some(health) = self.get(upstream_name, peer_url) {
            health.record_success();
        }
    }

    fn record_failure(&self, upstream_name: &str, peer_url: &str) -> PeerFailureStatus {
        let Some(policy) = self.policies.get(upstream_name).copied() else {
            return PeerFailureStatus { consecutive_failures: 0, entered_cooldown: false };
        };

        self.get(upstream_name, peer_url)
            .map(|health| health.record_failure(policy))
            .unwrap_or(PeerFailureStatus { consecutive_failures: 0, entered_cooldown: false })
    }

    fn is_available(&self, upstream_name: &str, peer_url: &str) -> bool {
        self.get(upstream_name, peer_url).is_none_or(|health| health.is_available())
    }

    fn record_active_success(
        &self,
        upstream_name: &str,
        peer_url: &str,
        healthy_successes_required: u32,
    ) -> ActiveProbeStatus {
        self.get(upstream_name, peer_url)
            .map(|health| health.record_active_success(healthy_successes_required))
            .unwrap_or(ActiveProbeStatus {
                healthy: true,
                recovered: false,
                consecutive_successes: 0,
            })
    }

    fn record_active_failure(&self, upstream_name: &str, peer_url: &str) -> bool {
        self.get(upstream_name, peer_url).is_some_and(|health| health.record_active_failure())
    }

    fn get(&self, upstream_name: &str, peer_url: &str) -> Option<&Arc<PeerHealth>> {
        self.peers.get(&PeerHealthKey {
            upstream_name: upstream_name.to_string(),
            peer_url: peer_url.to_string(),
        })
    }

    fn snapshot(
        &self,
        upstream_name: &str,
        peer_url: &str,
        peer_display_url: &str,
    ) -> PeerStatusSnapshot {
        self.get(upstream_name, peer_url)
            .map(|health| health.snapshot(peer_display_url))
            .unwrap_or_else(|| PeerStatusSnapshot {
                url: peer_display_url.to_string(),
                healthy: true,
                passive_consecutive_failures: 0,
                passive_cooldown_remaining_ms: None,
                active_unhealthy: false,
                active_consecutive_successes: 0,
            })
    }
}

impl PeerHealth {
    fn is_available(&self) -> bool {
        let mut state = lock_peer_health(&self.state);
        match state.passive.unhealthy_until {
            Some(until) if until > Instant::now() => false,
            Some(_) => {
                state.passive = PassiveHealthState::default();
                !state.active.unhealthy
            }
            None => !state.active.unhealthy,
        }
    }

    fn record_success(&self) {
        lock_peer_health(&self.state).passive = PassiveHealthState::default();
    }

    fn record_failure(&self, policy: PeerHealthPolicy) -> PeerFailureStatus {
        let mut state = lock_peer_health(&self.state);
        if state.passive.unhealthy_until.is_some_and(|until| until <= Instant::now()) {
            state.passive = PassiveHealthState::default();
        }

        state.passive.consecutive_failures += 1;
        let entered_cooldown =
            state.passive.consecutive_failures >= policy.unhealthy_after_failures;
        if entered_cooldown {
            state.passive.unhealthy_until = Some(Instant::now() + policy.cooldown);
        }

        PeerFailureStatus {
            consecutive_failures: state.passive.consecutive_failures,
            entered_cooldown,
        }
    }

    fn record_active_success(&self, healthy_successes_required: u32) -> ActiveProbeStatus {
        let mut state = lock_peer_health(&self.state);
        if !state.active.unhealthy {
            return ActiveProbeStatus { healthy: true, recovered: false, consecutive_successes: 0 };
        }

        state.active.consecutive_successes += 1;
        let consecutive_successes = state.active.consecutive_successes;
        let recovered = consecutive_successes >= healthy_successes_required;
        if recovered {
            state.active = ActiveHealthState::default();
        }

        ActiveProbeStatus { healthy: recovered, recovered, consecutive_successes }
    }

    fn record_active_failure(&self) -> bool {
        let mut state = lock_peer_health(&self.state);
        let was_healthy = !state.active.unhealthy;
        state.active.unhealthy = true;
        state.active.consecutive_successes = 0;
        was_healthy
    }

    fn snapshot(&self, url: &str) -> PeerStatusSnapshot {
        let mut state = lock_peer_health(&self.state);
        let now = Instant::now();

        if state.passive.unhealthy_until.is_some_and(|until| until <= now) {
            state.passive = PassiveHealthState::default();
        }

        let passive_cooldown_remaining_ms = state
            .passive
            .unhealthy_until
            .and_then(|until| until.checked_duration_since(now))
            .map(|remaining| remaining.as_millis() as u64);
        let healthy = passive_cooldown_remaining_ms.is_none() && !state.active.unhealthy;

        PeerStatusSnapshot {
            url: url.to_string(),
            healthy,
            passive_consecutive_failures: state.passive.consecutive_failures,
            passive_cooldown_remaining_ms,
            active_unhealthy: state.active.unhealthy,
            active_consecutive_successes: state.active.consecutive_successes,
        }
    }
}

fn lock_peer_health(state: &Mutex<PeerHealthState>) -> std::sync::MutexGuard<'_, PeerHealthState> {
    state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl PreparedProxyRequest {
    async fn from_request(
        request: Request<Incoming>,
        max_replayable_request_body_bytes: usize,
        max_request_body_bytes: Option<usize>,
    ) -> Result<Self, PrepareRequestError> {
        let (parts, body) = request.into_parts();
        let replayable = prepare_request_body(
            &parts.method,
            body,
            max_replayable_request_body_bytes,
            max_request_body_bytes,
        )
        .await?;

        Ok(Self {
            method: parts.method,
            version: parts.version,
            uri: parts.uri,
            headers: parts.headers,
            body: replayable,
        })
    }

    fn can_failover(&self) -> bool {
        is_idempotent_method(&self.method)
            && matches!(self.body, PreparedRequestBody::Replayable(_))
    }

    fn build_for_peer(
        &mut self,
        peer: &UpstreamPeer,
        upstream_name: &str,
        client_address: &ClientAddress,
        forwarded_proto: &str,
        preserve_host: bool,
        strip_prefix: Option<&str>,
        proxy_set_headers: &[(HeaderName, HeaderValue)],
    ) -> Result<Request<HttpBody>, Box<dyn std::error::Error + Send + Sync>> {
        let original_host = self.headers.get(HOST).cloned();
        let mut headers = self.headers.clone();
        let uri = build_proxy_uri(peer, &self.uri, strip_prefix)?;
        sanitize_request_headers(
            &mut headers,
            &peer.authority,
            original_host,
            client_address,
            forwarded_proto,
            preserve_host,
            proxy_set_headers,
        )?;

        tracing::debug!(
            upstream = %upstream_name,
            peer = %peer.url,
            uri = %uri,
            "forwarding request to upstream"
        );

        let mut request = Request::new(match &mut self.body {
            PreparedRequestBody::Replayable(body) => full_body(body.clone()),
            PreparedRequestBody::Streaming(body) => {
                streaming_request_body(body.take().ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "streaming request body is no longer available for replay",
                    )
                })?)
            }
        });
        *request.method_mut() = self.method.clone();
        *request.version_mut() = self.version;
        *request.uri_mut() = uri;
        *request.headers_mut() = headers;
        Ok(request)
    }
}

pub async fn probe_upstream_peer(
    clients: ProxyClients,
    metrics: Metrics,
    upstream: Arc<Upstream>,
    peer: UpstreamPeer,
) {
    let Some(check) = upstream.active_health_check.as_ref() else {
        return;
    };

    let client = match clients.for_upstream(upstream.as_ref()) {
        Ok(client) => client,
        Err(error) => {
            let transitioned = clients.record_active_peer_failure(&upstream.name, &peer.url);
            metrics.record_active_health_check(&upstream.name, &peer.url, "client_unavailable");
            let level = if transitioned { "unhealthy" } else { "still unhealthy" };
            tracing::warn!(
                upstream = %upstream.name,
                peer = %peer.url,
                %error,
                state = level,
                "active health check could not acquire a proxy client"
            );
            return;
        }
    };

    let request = match build_active_health_request(&peer, &check.path) {
        Ok(request) => request,
        Err(error) => {
            let transitioned = clients.record_active_peer_failure(&upstream.name, &peer.url);
            metrics.record_active_health_check(&upstream.name, &peer.url, "request_build_error");
            let level = if transitioned { "unhealthy" } else { "still unhealthy" };
            tracing::warn!(
                upstream = %upstream.name,
                peer = %peer.url,
                path = %check.path,
                %error,
                state = level,
                "active health check request could not be built"
            );
            return;
        }
    };

    match tokio::time::timeout(check.timeout, client.request(request)).await {
        Ok(Ok(response)) if response.status().is_success() => {
            metrics.record_active_health_check(&upstream.name, &peer.url, "healthy");
            let status = clients.record_active_peer_success(
                &upstream.name,
                &peer.url,
                check.healthy_successes_required,
            );
            if status.recovered {
                tracing::info!(
                    upstream = %upstream.name,
                    peer = %peer.url,
                    path = %check.path,
                    consecutive_successes = status.consecutive_successes,
                    "active health check marked peer healthy"
                );
            }
        }
        Ok(Ok(response)) => {
            let transitioned = clients.record_active_peer_failure(&upstream.name, &peer.url);
            metrics.record_active_health_check(&upstream.name, &peer.url, "unhealthy_status");
            tracing::warn!(
                upstream = %upstream.name,
                peer = %peer.url,
                path = %check.path,
                status = response.status().as_u16(),
                state = if transitioned { "unhealthy" } else { "still unhealthy" },
                "active health check returned an unhealthy status"
            );
        }
        Ok(Err(error)) => {
            let transitioned = clients.record_active_peer_failure(&upstream.name, &peer.url);
            metrics.record_active_health_check(&upstream.name, &peer.url, "request_error");
            tracing::warn!(
                upstream = %upstream.name,
                peer = %peer.url,
                path = %check.path,
                %error,
                state = if transitioned { "unhealthy" } else { "still unhealthy" },
                "active health check request failed"
            );
        }
        Err(_) => {
            let transitioned = clients.record_active_peer_failure(&upstream.name, &peer.url);
            metrics.record_active_health_check(&upstream.name, &peer.url, "timeout");
            tracing::warn!(
                upstream = %upstream.name,
                peer = %peer.url,
                path = %check.path,
                timeout_ms = check.timeout.as_millis() as u64,
                state = if transitioned { "unhealthy" } else { "still unhealthy" },
                "active health check timed out"
            );
        }
    }
}

pub async fn forward_request(
    clients: ProxyClients,
    metrics: Metrics,
    request: Request<Incoming>,
    target: &ProxyTarget,
    client_address: ClientAddress,
    downstream_proto: &str,
    max_request_body_bytes: Option<usize>,
) -> HttpResponse {
    let client = match clients.for_upstream(target.upstream.as_ref()) {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(
                upstream = %target.upstream_name,
                %error,
                "failed to select proxy client"
            );
            return bad_gateway(format!(
                "upstream `{}` TLS client is unavailable\n",
                target.upstream_name
            ));
        }
    };

    let mut prepared_request = match PreparedProxyRequest::from_request(
        request,
        target.upstream.max_replayable_request_body_bytes,
        max_request_body_bytes,
    )
    .await
    {
        Ok(request) => request,
        Err(PrepareRequestError::PayloadTooLarge { max_request_body_bytes }) => {
            tracing::info!(
                upstream = %target.upstream_name,
                max_request_body_bytes,
                "rejecting request body that exceeds configured server limit"
            );
            return payload_too_large(format!(
                "request body exceeds server.max_request_body_bytes ({max_request_body_bytes} bytes)\n"
            ));
        }
        Err(error) => {
            tracing::warn!(
                upstream = %target.upstream_name,
                %error,
                "failed to prepare upstream request"
            );
            return bad_gateway(format!(
                "failed to prepare upstream request for `{}`\n",
                target.upstream_name
            ));
        }
    };
    let can_failover = prepared_request.can_failover();
    let selected = clients.select_peers(
        target.upstream.as_ref(),
        if can_failover { MAX_FAILOVER_ATTEMPTS } else { 1 },
    );
    let peers = selected.peers;
    if peers.is_empty() {
        tracing::warn!(
            upstream = %target.upstream_name,
            skipped_unhealthy = selected.skipped_unhealthy,
            "proxy route has no healthy peers available"
        );
        return bad_gateway(format!(
            "upstream `{}` has no healthy peers available\n",
            target.upstream_name
        ));
    }

    for (attempt_index, peer) in peers.iter().enumerate() {
        let upstream_request = match prepared_request.build_for_peer(
            peer,
            &target.upstream_name,
            &client_address,
            downstream_proto,
            target.preserve_host,
            target.strip_prefix.as_deref(),
            &target.proxy_set_headers,
        ) {
            Ok(request) => request,
            Err(error) => {
                tracing::warn!(
                    upstream = %target.upstream_name,
                    peer = %peer.url,
                    %error,
                    "failed to build upstream request"
                );
                return bad_gateway(format!(
                    "failed to build upstream request for `{}`\n",
                    target.upstream_name
                ));
            }
        };

        match wait_for_upstream_stage(
            target.upstream.request_timeout,
            &target.upstream_name,
            "request",
            client.request(upstream_request),
        )
        .await
        {
            Ok(Ok(response)) => {
                clients.record_peer_success(&target.upstream_name, &peer.url);
                metrics.record_upstream_request(&target.upstream_name, &peer.url, "success");
                if attempt_index > 0 {
                    tracing::info!(
                        upstream = %target.upstream_name,
                        peer = %peer.url,
                        attempt = attempt_index + 1,
                        "upstream failover request succeeded"
                    );
                }
                return build_downstream_response(
                    response,
                    &target.upstream_name,
                    &peer.url,
                    target.upstream.request_timeout,
                );
            }
            Ok(Err(error)) if can_retry_peer_request(&prepared_request, &peers, attempt_index) => {
                let failure = clients.record_peer_failure(&target.upstream_name, &peer.url);
                metrics.record_upstream_request(&target.upstream_name, &peer.url, "error");
                let next_peer = &peers[attempt_index + 1];
                tracing::warn!(
                    upstream = %target.upstream_name,
                    failed_peer = %peer.url,
                    next_peer = %next_peer.url,
                    attempt = attempt_index + 1,
                    consecutive_failures = failure.consecutive_failures,
                    entered_cooldown = failure.entered_cooldown,
                    %error,
                    "retrying idempotent upstream request on alternate peer"
                );
            }
            Ok(Err(error)) => {
                let failure = clients.record_peer_failure(&target.upstream_name, &peer.url);
                metrics.record_upstream_request(&target.upstream_name, &peer.url, "error");
                tracing::warn!(
                    upstream = %target.upstream_name,
                    peer = %peer.url,
                    consecutive_failures = failure.consecutive_failures,
                    entered_cooldown = failure.entered_cooldown,
                    %error,
                    "upstream request failed"
                );
                return bad_gateway(format!(
                    "upstream `{}` is unavailable\n",
                    target.upstream_name
                ));
            }
            Err(error) if can_retry_peer_request(&prepared_request, &peers, attempt_index) => {
                let failure = clients.record_peer_failure(&target.upstream_name, &peer.url);
                metrics.record_upstream_request(&target.upstream_name, &peer.url, "timeout");
                let next_peer = &peers[attempt_index + 1];
                tracing::warn!(
                    upstream = %target.upstream_name,
                    failed_peer = %peer.url,
                    next_peer = %next_peer.url,
                    attempt = attempt_index + 1,
                    timeout_ms = target.upstream.request_timeout.as_millis() as u64,
                    consecutive_failures = failure.consecutive_failures,
                    entered_cooldown = failure.entered_cooldown,
                    %error,
                    "retrying idempotent upstream request on alternate peer after timeout"
                );
            }
            Err(error) => {
                let failure = clients.record_peer_failure(&target.upstream_name, &peer.url);
                metrics.record_upstream_request(&target.upstream_name, &peer.url, "timeout");
                tracing::warn!(
                    upstream = %target.upstream_name,
                    peer = %peer.url,
                    timeout_ms = target.upstream.request_timeout.as_millis() as u64,
                    consecutive_failures = failure.consecutive_failures,
                    entered_cooldown = failure.entered_cooldown,
                    %error,
                    "upstream request timed out"
                );
                return gateway_timeout(format!(
                    "upstream `{}` timed out after {} ms\n",
                    target.upstream_name,
                    target.upstream.request_timeout.as_millis()
                ));
            }
        }
    }

    bad_gateway(format!("upstream `{}` is unavailable\n", target.upstream_name))
}

fn build_active_health_request(
    peer: &UpstreamPeer,
    path: &str,
) -> Result<Request<HttpBody>, Error> {
    let path: Uri = path.parse().map_err(|error| {
        Error::Server(format!(
            "failed to parse active health-check path `{path}` for peer `{}`: {error}",
            peer.url
        ))
    })?;
    let uri = build_proxy_uri(peer, &path, None).map_err(|error| {
        Error::Server(format!("failed to build active health-check uri: {error}"))
    })?;

    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(HOST, peer.authority.as_str())
        .body(full_body(Bytes::new()))
        .map_err(|error| {
            Error::Server(format!("failed to build active health-check request: {error}"))
        })
}

async fn wait_for_upstream_stage<T>(
    request_timeout: Duration,
    upstream_name: &str,
    stage: &str,
    future: impl Future<Output = T>,
) -> Result<T, Error> {
    tokio::time::timeout(request_timeout, future).await.map_err(|_| {
        Error::Server(format!(
            "upstream `{upstream_name}` {stage} timed out after {} ms",
            request_timeout.as_millis()
        ))
    })
}

fn gateway_timeout(message: String) -> HttpResponse {
    crate::handler::text_response(StatusCode::GATEWAY_TIMEOUT, "text/plain; charset=utf-8", message)
}

fn bad_gateway(message: String) -> HttpResponse {
    crate::handler::text_response(StatusCode::BAD_GATEWAY, "text/plain; charset=utf-8", message)
}

fn payload_too_large(message: String) -> HttpResponse {
    crate::handler::text_response(
        StatusCode::PAYLOAD_TOO_LARGE,
        "text/plain; charset=utf-8",
        message,
    )
}

async fn prepare_request_body(
    method: &Method,
    body: Incoming,
    max_replayable_request_body_bytes: usize,
    max_request_body_bytes: Option<usize>,
) -> Result<PreparedRequestBody, PrepareRequestError> {
    if let Some(max_request_body_bytes) = max_request_body_bytes {
        if body.is_end_stream() {
            return Ok(PreparedRequestBody::Replayable(Bytes::new()));
        }

        if body.size_hint().lower() > max_request_body_bytes as u64 {
            return Err(PrepareRequestError::payload_too_large(max_request_body_bytes));
        }

        let body = collect_request_body_with_limit(body, max_request_body_bytes).await?;
        return Ok(PreparedRequestBody::Replayable(body));
    }

    if !is_idempotent_method(method) {
        return Ok(PreparedRequestBody::Streaming(Some(body)));
    }

    if body.is_end_stream() {
        return Ok(PreparedRequestBody::Replayable(Bytes::new()));
    }

    match body.size_hint().upper() {
        Some(upper) if upper <= max_replayable_request_body_bytes as u64 => {
            let body = body.collect().await.map_err(PrepareRequestError::other)?.to_bytes();
            Ok(PreparedRequestBody::Replayable(body))
        }
        _ => Ok(PreparedRequestBody::Streaming(Some(body))),
    }
}

async fn collect_request_body_with_limit(
    mut body: Incoming,
    max_request_body_bytes: usize,
) -> Result<Bytes, PrepareRequestError> {
    let mut collected = BytesMut::new();

    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(PrepareRequestError::other)?;
        let Ok(data) = frame.into_data() else {
            continue;
        };

        if data.len() > max_request_body_bytes.saturating_sub(collected.len()) {
            return Err(PrepareRequestError::payload_too_large(max_request_body_bytes));
        }
        collected.extend_from_slice(&data);
    }

    Ok(collected.freeze())
}

fn streaming_request_body(body: Incoming) -> HttpBody {
    body.map_err(|error| -> BoxError { Box::new(error) }).boxed_unsync()
}

fn is_idempotent_method(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::PUT | Method::DELETE | Method::OPTIONS | Method::TRACE
    )
}

fn can_retry_peer_request(
    prepared_request: &PreparedProxyRequest,
    peers: &[UpstreamPeer],
    attempt_index: usize,
) -> bool {
    prepared_request.can_failover() && attempt_index + 1 < peers.len()
}

fn build_downstream_response(
    response: Response<Incoming>,
    upstream_name: &str,
    peer_url: &str,
    request_timeout: Duration,
) -> HttpResponse {
    let (parts, body) = response.into_parts();
    let status = parts.status;
    let version = parts.version;
    let mut headers = parts.headers;
    sanitize_response_headers(&mut headers);

    let label = format!("upstream `{upstream_name}` response body from `{peer_url}`");
    let body = IdleTimeoutBody::new(body, request_timeout, label).boxed_unsync();

    let mut downstream = Response::new(body);
    *downstream.status_mut() = status;
    *downstream.version_mut() = version;
    *downstream.headers_mut() = headers;
    downstream
}

fn build_client_for_profile(profile: &TlsClientProfile) -> Result<ProxyClient, Error> {
    let mut connector = HttpConnector::new();
    connector.enforce_http(false);

    let tls_config = build_tls_config(&profile.tls)?;
    let builder = HttpsConnectorBuilder::new().with_tls_config(tls_config).https_or_http();
    let builder = if let Some(server_name_override) = &profile.server_name_override {
        let server_name = ServerName::try_from(server_name_override.clone()).map_err(|error| {
            Error::Server(format!(
                "invalid TLS server_name_override `{server_name_override}`: {error}"
            ))
        })?;
        builder.with_server_name_resolver(FixedServerNameResolver::new(server_name))
    } else {
        builder
    };
    let connector = builder.enable_http1().wrap_connector(connector);

    Ok(Client::builder(TokioExecutor::new()).build(connector))
}

fn build_tls_config(tls: &UpstreamTls) -> Result<ClientConfig, Error> {
    match tls {
        UpstreamTls::NativeRoots => {
            let builder = ClientConfig::builder().with_native_roots().map_err(|error| {
                Error::Server(format!("failed to load native TLS roots: {error}"))
            })?;
            Ok(builder.with_no_client_auth())
        }
        UpstreamTls::CustomCa { ca_cert_path } => {
            let roots = load_custom_ca_store(ca_cert_path)?;
            Ok(ClientConfig::builder().with_root_certificates(roots).with_no_client_auth())
        }
        UpstreamTls::Insecure => {
            let verifier = Arc::new(InsecureServerCertVerifier::new());
            Ok(ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(verifier)
                .with_no_client_auth())
        }
    }
}

fn load_custom_ca_store(path: &Path) -> Result<RootCertStore, Error> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let certs =
        rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>().map_err(|error| {
            Error::Server(format!(
                "failed to parse custom CA certificates from `{}`: {error}",
                path.display()
            ))
        })?;

    let mut roots = RootCertStore::empty();
    if certs.is_empty() {
        let der = std::fs::read(path)?;
        roots.add(CertificateDer::from(der)).map_err(|error| {
            Error::Server(format!(
                "failed to add DER custom CA certificate `{}`: {error}",
                path.display()
            ))
        })?;
        return Ok(roots);
    }

    let (added, _ignored) = roots.add_parsable_certificates(certs);
    if added == 0 || roots.is_empty() {
        return Err(Error::Server(format!(
            "no valid CA certificates were loaded from `{}`",
            path.display()
        )));
    }

    Ok(roots)
}

fn build_proxy_uri(
    peer: &UpstreamPeer,
    original_uri: &Uri,
    strip_prefix: Option<&str>,
) -> Result<Uri, http::Error> {
    let original_path = original_uri.path_and_query().map(|value| value.as_str()).unwrap_or("/");

    let path_and_query = if let Some(prefix) = strip_prefix {
        if let Some(stripped) = original_path.strip_prefix(prefix) {
            if stripped.is_empty() || stripped.starts_with('?') {
                if stripped.is_empty() { "/" } else { stripped }
            } else if stripped.starts_with('/') {
                stripped
            } else {
                original_path
            }
        } else {
            original_path
        }
    } else {
        original_path
    };

    Uri::builder()
        .scheme(peer.scheme.as_str())
        .authority(peer.authority.as_str())
        .path_and_query(path_and_query)
        .build()
}

fn sanitize_request_headers(
    headers: &mut HeaderMap,
    authority: &str,
    original_host: Option<HeaderValue>,
    client_address: &ClientAddress,
    forwarded_proto: &str,
    preserve_host: bool,
    proxy_set_headers: &[(HeaderName, HeaderValue)],
) -> Result<(), http::header::InvalidHeaderValue> {
    remove_hop_by_hop_headers(headers);

    if preserve_host {
        if let Some(ref host) = original_host {
            headers.insert(HOST, host.clone());
        } else {
            headers.insert(HOST, HeaderValue::from_str(authority)?);
        }
    } else {
        headers.insert(HOST, HeaderValue::from_str(authority)?);
    }

    headers.insert("x-forwarded-proto", HeaderValue::from_str(forwarded_proto)?);

    if let Some(host) = original_host {
        headers.insert("x-forwarded-host", host);
    }

    headers.insert("x-forwarded-for", HeaderValue::from_str(&client_address.forwarded_for)?);

    for (name, value) in proxy_set_headers {
        headers.insert(name.clone(), value.clone());
    }

    Ok(())
}

fn sanitize_response_headers(headers: &mut HeaderMap) {
    remove_hop_by_hop_headers(headers);
}

fn remove_hop_by_hop_headers(headers: &mut HeaderMap) {
    let mut extra_headers = Vec::new();

    for value in headers.get_all(CONNECTION) {
        if let Ok(value) = value.to_str() {
            for item in value.split(',') {
                let trimmed = item.trim();
                if trimmed.is_empty() {
                    continue;
                }

                if let Ok(name) = HeaderName::from_bytes(trimmed.as_bytes()) {
                    extra_headers.push(name);
                }
            }
        }
    }

    for name in extra_headers {
        headers.remove(name);
    }

    for name in [
        CONNECTION,
        PROXY_AUTHENTICATE,
        PROXY_AUTHORIZATION,
        TE,
        TRAILER,
        TRANSFER_ENCODING,
        UPGRADE,
    ] {
        headers.remove(name);
    }

    headers.remove("keep-alive");
    headers.remove("proxy-connection");
}

#[derive(Debug)]
struct InsecureServerCertVerifier {
    supported_schemes: Vec<SignatureScheme>,
}

impl InsecureServerCertVerifier {
    fn new() -> Self {
        let supported_schemes = rustls::crypto::aws_lc_rs::default_provider()
            .signature_verification_algorithms
            .supported_schemes();
        Self { supported_schemes }
    }
}

impl ServerCertVerifier for InsecureServerCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.supported_schemes.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::io::{Read, Write};
    use std::net::SocketAddr;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::thread;
    use std::time::Duration;
    use std::time::{SystemTime, UNIX_EPOCH};

    use bytes::Bytes;
    use http::header::HOST;
    use http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, Version};
    use rginx_core::{ActiveHealthCheck, Error, Upstream, UpstreamPeer, UpstreamTls};

    use super::{
        build_proxy_uri, can_retry_peer_request, is_idempotent_method, load_custom_ca_store,
        probe_upstream_peer, sanitize_request_headers, wait_for_upstream_stage,
        PreparedProxyRequest, PreparedRequestBody, ProxyClients,
    };
    use crate::client_ip::{ClientAddress, ClientIpSource};
    use crate::metrics::Metrics;

    #[test]
    fn proxy_uri_keeps_path_and_query() {
        let peer = UpstreamPeer {
            url: "http://127.0.0.1:9000".to_string(),
            scheme: "http".to_string(),
            authority: "127.0.0.1:9000".to_string(),
        };

        let uri = build_proxy_uri(&peer, &"/api/demo?x=1".parse().unwrap(), None).unwrap();
        assert_eq!(uri, "http://127.0.0.1:9000/api/demo?x=1".parse::<http::Uri>().unwrap());
    }

    #[test]
    fn proxy_uri_keeps_https_scheme() {
        let peer = UpstreamPeer {
            url: "https://example.com".to_string(),
            scheme: "https".to_string(),
            authority: "example.com".to_string(),
        };

        let uri = build_proxy_uri(&peer, &"/healthz".parse().unwrap(), None).unwrap();
        assert_eq!(uri, "https://example.com/healthz".parse::<http::Uri>().unwrap());
    }

    #[test]
    fn sanitize_request_headers_overwrites_x_forwarded_for_with_sanitized_chain() {
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("client.example"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("spoofed"));

        let client_address = ClientAddress {
            peer_addr: "10.2.3.4:4000".parse().unwrap(),
            client_ip: "198.51.100.9".parse().unwrap(),
            forwarded_for: "198.51.100.9, 10.1.2.3, 10.2.3.4".to_string(),
            source: ClientIpSource::XForwardedFor,
        };

        sanitize_request_headers(
            &mut headers,
            "127.0.0.1:9000",
            Some(HeaderValue::from_static("client.example")),
            &client_address,
            "https",
            false,
            &[],
        )
        .expect("header sanitization should succeed");

        assert_eq!(headers.get(HOST).unwrap(), "127.0.0.1:9000");
        assert_eq!(headers.get("x-forwarded-host").unwrap(), "client.example");
        assert_eq!(headers.get("x-forwarded-for").unwrap(), "198.51.100.9, 10.1.2.3, 10.2.3.4");
        assert_eq!(headers.get("x-forwarded-proto").unwrap(), "https");
    }

    #[test]
    fn load_custom_ca_store_accepts_pem_files() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("rginx-custom-ca-{unique}.pem"));
        std::fs::write(&path, TEST_CA_CERT_PEM).expect("PEM file should be written");

        let store = load_custom_ca_store(&path).expect("PEM CA should load");
        assert!(!store.is_empty());

        std::fs::remove_file(path).expect("temp PEM file should be removed");
    }

    #[test]
    fn proxy_clients_can_select_insecure_and_custom_ca_modes() {
        let insecure = Upstream::new(
            "insecure".to_string(),
            vec![UpstreamPeer {
                url: "https://localhost:9443".to_string(),
                scheme: "https".to_string(),
                authority: "localhost:9443".to_string(),
            }],
            UpstreamTls::Insecure,
            None,
            Duration::from_secs(30),
            64 * 1024,
            2,
            Duration::from_secs(10),
            None,
        );

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("rginx-custom-ca-select-{unique}.pem"));
        std::fs::write(&path, TEST_CA_CERT_PEM).expect("PEM file should be written");

        let custom = Upstream::new(
            "custom".to_string(),
            vec![UpstreamPeer {
                url: "https://localhost:9443".to_string(),
                scheme: "https".to_string(),
                authority: "localhost:9443".to_string(),
            }],
            UpstreamTls::CustomCa { ca_cert_path: path.clone() },
            None,
            Duration::from_secs(30),
            64 * 1024,
            2,
            Duration::from_secs(10),
            None,
        );

        let snapshot = rginx_core::ConfigSnapshot {
            runtime: rginx_core::RuntimeSettings {
                shutdown_timeout: std::time::Duration::from_secs(1),
            },
            server: rginx_core::Server {
                listen_addr: "127.0.0.1:8080".parse().unwrap(),
                trusted_proxies: Vec::new(),
                keep_alive: true,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout: None,
                tls: None,
            },
            default_vhost: rginx_core::VirtualHost {
                server_names: Vec::new(),
                routes: Vec::new(),
                tls: None,
            },
            vhosts: Vec::new(),
            routes: Vec::new(),
            upstreams: HashMap::from([
                ("insecure".to_string(), Arc::new(insecure)),
                ("custom".to_string(), Arc::new(custom)),
            ]),
        };

        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");
        assert!(clients.for_upstream(snapshot.upstreams["insecure"].as_ref()).is_ok());
        assert!(clients.for_upstream(snapshot.upstreams["custom"].as_ref()).is_ok());

        std::fs::remove_file(path).expect("temp PEM file should be removed");
    }

    #[test]
    fn proxy_clients_cache_distinguishes_server_name_override() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("rginx-custom-ca-override-{unique}.pem"));
        std::fs::write(&path, TEST_CA_CERT_PEM).expect("PEM file should be written");

        let peer = UpstreamPeer {
            url: "https://127.0.0.1:9443".to_string(),
            scheme: "https".to_string(),
            authority: "127.0.0.1:9443".to_string(),
        };
        let first = Upstream::new(
            "first".to_string(),
            vec![peer.clone()],
            UpstreamTls::CustomCa { ca_cert_path: path.clone() },
            Some("api-a.internal".to_string()),
            Duration::from_secs(30),
            64 * 1024,
            2,
            Duration::from_secs(10),
            None,
        );
        let second = Upstream::new(
            "second".to_string(),
            vec![peer.clone()],
            UpstreamTls::CustomCa { ca_cert_path: path.clone() },
            Some("api-b.internal".to_string()),
            Duration::from_secs(30),
            64 * 1024,
            2,
            Duration::from_secs(10),
            None,
        );
        let duplicate = Upstream::new(
            "duplicate".to_string(),
            vec![peer],
            UpstreamTls::CustomCa { ca_cert_path: path.clone() },
            Some("api-a.internal".to_string()),
            Duration::from_secs(30),
            64 * 1024,
            2,
            Duration::from_secs(10),
            None,
        );

        let snapshot = rginx_core::ConfigSnapshot {
            runtime: rginx_core::RuntimeSettings {
                shutdown_timeout: std::time::Duration::from_secs(1),
            },
            server: rginx_core::Server {
                listen_addr: "127.0.0.1:8080".parse().unwrap(),
                trusted_proxies: Vec::new(),
                keep_alive: true,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout: None,
                tls: None,
            },
            default_vhost: rginx_core::VirtualHost {
                server_names: Vec::new(),
                routes: Vec::new(),
                tls: None,
            },
            vhosts: Vec::new(),
            routes: Vec::new(),
            upstreams: HashMap::from([
                ("first".to_string(), Arc::new(first)),
                ("second".to_string(), Arc::new(second)),
                ("duplicate".to_string(), Arc::new(duplicate)),
            ]),
        };

        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");
        assert_eq!(clients.clients.len(), 2);
        assert!(clients.for_upstream(snapshot.upstreams["first"].as_ref()).is_ok());
        assert!(clients.for_upstream(snapshot.upstreams["second"].as_ref()).is_ok());
        assert!(clients.for_upstream(snapshot.upstreams["duplicate"].as_ref()).is_ok());

        std::fs::remove_file(path).expect("temp PEM file should be removed");
    }

    #[tokio::test]
    async fn wait_for_upstream_stage_times_out() {
        let timeout = Duration::from_millis(25);

        let error = wait_for_upstream_stage(timeout, "backend", "request", async {
            tokio::time::sleep(Duration::from_millis(100)).await;
        })
        .await
        .expect_err("slow future should time out");

        assert!(matches!(error, Error::Server(message) if message.contains("timed out")));
    }

    #[test]
    fn upstream_next_peers_returns_distinct_failover_candidates() {
        let upstream = Upstream::new(
            "backend".to_string(),
            vec![
                peer("http://127.0.0.1:9000"),
                peer("http://127.0.0.1:9001"),
                peer("http://127.0.0.1:9002"),
            ],
            UpstreamTls::NativeRoots,
            None,
            Duration::from_secs(30),
            64 * 1024,
            2,
            Duration::from_secs(10),
            None,
        );

        let first = upstream.next_peers(2);
        let second = upstream.next_peers(2);

        assert_eq!(
            first.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
            vec!["http://127.0.0.1:9000", "http://127.0.0.1:9001",]
        );
        assert_eq!(
            second.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
            vec!["http://127.0.0.1:9001", "http://127.0.0.1:9002",]
        );
    }

    #[test]
    fn replayable_idempotent_requests_retry_once() {
        let prepared = PreparedProxyRequest {
            method: Method::GET,
            version: Version::HTTP_11,
            uri: Uri::from_static("/"),
            headers: HeaderMap::new(),
            body: PreparedRequestBody::Replayable(Bytes::new()),
        };
        let peers = vec![peer("http://127.0.0.1:9000"), peer("http://127.0.0.1:9001")];

        assert!(can_retry_peer_request(&prepared, &peers, 0));
        assert!(!can_retry_peer_request(&prepared, &peers, 1));
    }

    #[test]
    fn streaming_requests_do_not_retry() {
        let prepared = PreparedProxyRequest {
            method: Method::GET,
            version: Version::HTTP_11,
            uri: Uri::from_static("/"),
            headers: HeaderMap::new(),
            body: PreparedRequestBody::Streaming(None),
        };
        let peers = vec![peer("http://127.0.0.1:9000"), peer("http://127.0.0.1:9001")];

        assert!(!can_retry_peer_request(&prepared, &peers, 0));
    }

    #[test]
    fn idempotent_method_detection_matches_retry_policy() {
        assert!(is_idempotent_method(&Method::GET));
        assert!(is_idempotent_method(&Method::PUT));
        assert!(is_idempotent_method(&Method::DELETE));
        assert!(!is_idempotent_method(&Method::POST));
        assert!(!is_idempotent_method(&Method::PATCH));
    }

    #[test]
    fn unhealthy_peer_is_skipped_after_consecutive_failures() {
        let snapshot = snapshot_with_upstream_policy(
            "backend",
            vec![peer("http://127.0.0.1:9000"), peer("http://127.0.0.1:9001")],
            2,
            Duration::from_secs(30),
        );
        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

        let first = clients.select_peers(snapshot.upstreams["backend"].as_ref(), 2);
        assert_eq!(first.skipped_unhealthy, 0);
        assert_eq!(
            first.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
            vec!["http://127.0.0.1:9000", "http://127.0.0.1:9001"]
        );

        let first_failure = clients.record_peer_failure("backend", "http://127.0.0.1:9000");
        assert_eq!(first_failure.consecutive_failures, 1);
        assert!(!first_failure.entered_cooldown);

        let second_failure = clients.record_peer_failure("backend", "http://127.0.0.1:9000");
        assert_eq!(second_failure.consecutive_failures, 2);
        assert!(second_failure.entered_cooldown);

        let selected = clients.select_peers(snapshot.upstreams["backend"].as_ref(), 2);
        assert_eq!(selected.skipped_unhealthy, 1);
        assert_eq!(
            selected.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
            vec!["http://127.0.0.1:9001"]
        );
    }

    #[tokio::test]
    async fn unhealthy_peer_recovers_after_cooldown() {
        let snapshot = snapshot_with_upstream_policy(
            "backend",
            vec![peer("http://127.0.0.1:9000"), peer("http://127.0.0.1:9001")],
            1,
            Duration::from_millis(20),
        );
        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

        let failure = clients.record_peer_failure("backend", "http://127.0.0.1:9000");
        assert!(failure.entered_cooldown);

        let immediately = clients.select_peers(snapshot.upstreams["backend"].as_ref(), 2);
        assert_eq!(immediately.skipped_unhealthy, 1);
        assert_eq!(
            immediately.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
            vec!["http://127.0.0.1:9001"]
        );

        tokio::time::sleep(Duration::from_millis(30)).await;

        let recovered = clients.select_peers(snapshot.upstreams["backend"].as_ref(), 2);
        assert_eq!(recovered.skipped_unhealthy, 0);
        assert_eq!(recovered.peers.len(), 2);
    }

    #[test]
    fn successful_request_resets_peer_failure_count() {
        let snapshot = snapshot_with_upstream_policy(
            "backend",
            vec![peer("http://127.0.0.1:9000")],
            2,
            Duration::from_secs(30),
        );
        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

        let failure = clients.record_peer_failure("backend", "http://127.0.0.1:9000");
        assert_eq!(failure.consecutive_failures, 1);

        clients.record_peer_success("backend", "http://127.0.0.1:9000");

        let after_reset = clients.record_peer_failure("backend", "http://127.0.0.1:9000");
        assert_eq!(after_reset.consecutive_failures, 1);
        assert!(!after_reset.entered_cooldown);
    }

    #[test]
    fn peer_health_policy_is_applied_per_upstream() {
        let fast_fail = Arc::new(Upstream::new(
            "fast-fail".to_string(),
            vec![peer("http://127.0.0.1:9000")],
            UpstreamTls::NativeRoots,
            None,
            Duration::from_secs(30),
            64 * 1024,
            1,
            Duration::from_secs(30),
            None,
        ));
        let tolerant = Arc::new(Upstream::new(
            "tolerant".to_string(),
            vec![peer("http://127.0.0.1:9010")],
            UpstreamTls::NativeRoots,
            None,
            Duration::from_secs(30),
            64 * 1024,
            3,
            Duration::from_secs(30),
            None,
        ));

        let snapshot = rginx_core::ConfigSnapshot {
            runtime: rginx_core::RuntimeSettings { shutdown_timeout: Duration::from_secs(1) },
            server: rginx_core::Server {
                listen_addr: "127.0.0.1:8080".parse().unwrap(),
                trusted_proxies: Vec::new(),
                keep_alive: true,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout: None,
                tls: None,
            },
            default_vhost: rginx_core::VirtualHost {
                server_names: Vec::new(),
                routes: Vec::new(),
                tls: None,
            },
            vhosts: Vec::new(),
            routes: Vec::new(),
            upstreams: HashMap::from([
                ("fast-fail".to_string(), fast_fail.clone()),
                ("tolerant".to_string(), tolerant.clone()),
            ]),
        };
        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

        let fast_failure = clients.record_peer_failure("fast-fail", "http://127.0.0.1:9000");
        assert!(fast_failure.entered_cooldown);

        let tolerant_failure = clients.record_peer_failure("tolerant", "http://127.0.0.1:9010");
        assert_eq!(tolerant_failure.consecutive_failures, 1);
        assert!(!tolerant_failure.entered_cooldown);

        let fast_selected = clients.select_peers(snapshot.upstreams["fast-fail"].as_ref(), 1);
        assert!(fast_selected.peers.is_empty());
        assert_eq!(fast_selected.skipped_unhealthy, 1);

        let tolerant_selected = clients.select_peers(snapshot.upstreams["tolerant"].as_ref(), 1);
        assert_eq!(tolerant_selected.peers.len(), 1);
        assert_eq!(tolerant_selected.skipped_unhealthy, 0);
    }

    #[test]
    fn active_health_requires_recovery_threshold_before_peer_is_reused() {
        let snapshot = snapshot_with_active_health(
            "backend",
            vec![peer("http://127.0.0.1:9000")],
            "/healthz",
            2,
        );
        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

        assert_eq!(clients.select_peers(snapshot.upstreams["backend"].as_ref(), 1).peers.len(), 1);
        assert!(clients.record_active_peer_failure("backend", "http://127.0.0.1:9000"));
        assert!(clients.select_peers(snapshot.upstreams["backend"].as_ref(), 1).peers.is_empty());

        let first_success =
            clients.record_active_peer_success("backend", "http://127.0.0.1:9000", 2);
        assert!(!first_success.recovered);
        assert_eq!(first_success.consecutive_successes, 1);
        assert!(clients.select_peers(snapshot.upstreams["backend"].as_ref(), 1).peers.is_empty());

        let second_success =
            clients.record_active_peer_success("backend", "http://127.0.0.1:9000", 2);
        assert!(second_success.recovered);
        assert_eq!(second_success.consecutive_successes, 2);
        assert_eq!(clients.select_peers(snapshot.upstreams["backend"].as_ref(), 1).peers.len(), 1);
    }

    #[tokio::test]
    async fn active_health_probe_tracks_status_transitions() {
        let statuses = Arc::new(Mutex::new(VecDeque::from([
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::OK,
            StatusCode::OK,
        ])));
        let listen_addr = spawn_status_server(statuses).await;
        let peer_url = format!("http://{listen_addr}");
        let snapshot = snapshot_with_active_health("backend", vec![peer(&peer_url)], "/healthz", 2);
        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");
        let metrics = Metrics::default();
        let upstream = snapshot.upstreams["backend"].clone();
        let peer = upstream.peers[0].clone();

        probe_upstream_peer(clients.clone(), metrics.clone(), upstream.clone(), peer.clone()).await;
        assert!(clients.select_peers(upstream.as_ref(), 1).peers.is_empty());

        probe_upstream_peer(clients.clone(), metrics.clone(), upstream.clone(), peer.clone()).await;
        assert!(clients.select_peers(upstream.as_ref(), 1).peers.is_empty());

        probe_upstream_peer(clients.clone(), metrics.clone(), upstream.clone(), peer).await;
        assert_eq!(clients.select_peers(upstream.as_ref(), 1).peers.len(), 1);
        let rendered = metrics.render_prometheus();
        assert!(rendered.contains("rginx_active_health_checks_total"));
    }

    fn peer(url: &str) -> UpstreamPeer {
        let uri: http::Uri = url.parse().expect("peer URL should parse");
        UpstreamPeer {
            url: url.to_string(),
            scheme: uri.scheme_str().expect("peer should have scheme").to_string(),
            authority: uri.authority().expect("peer should have authority").to_string(),
        }
    }

    fn snapshot_with_upstream_policy(
        name: &str,
        peers: Vec<UpstreamPeer>,
        unhealthy_after_failures: u32,
        unhealthy_cooldown: Duration,
    ) -> rginx_core::ConfigSnapshot {
        let upstream = Upstream::new(
            name.to_string(),
            peers,
            UpstreamTls::NativeRoots,
            None,
            Duration::from_secs(30),
            64 * 1024,
            unhealthy_after_failures,
            unhealthy_cooldown,
            None,
        );

        rginx_core::ConfigSnapshot {
            runtime: rginx_core::RuntimeSettings { shutdown_timeout: Duration::from_secs(1) },
            server: rginx_core::Server {
                listen_addr: "127.0.0.1:8080".parse().unwrap(),
                trusted_proxies: Vec::new(),
                keep_alive: true,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout: None,
                tls: None,
            },
            default_vhost: rginx_core::VirtualHost {
                server_names: Vec::new(),
                routes: Vec::new(),
                tls: None,
            },
            vhosts: Vec::new(),
            routes: Vec::new(),
            upstreams: HashMap::from([(name.to_string(), Arc::new(upstream))]),
        }
    }

    fn snapshot_with_active_health(
        name: &str,
        peers: Vec<UpstreamPeer>,
        path: &str,
        healthy_successes_required: u32,
    ) -> rginx_core::ConfigSnapshot {
        let upstream = Upstream::new(
            name.to_string(),
            peers,
            UpstreamTls::NativeRoots,
            None,
            Duration::from_secs(30),
            64 * 1024,
            2,
            Duration::from_secs(10),
            Some(ActiveHealthCheck {
                path: path.to_string(),
                interval: Duration::from_secs(5),
                timeout: Duration::from_secs(1),
                healthy_successes_required,
            }),
        );

        rginx_core::ConfigSnapshot {
            runtime: rginx_core::RuntimeSettings { shutdown_timeout: Duration::from_secs(1) },
            server: rginx_core::Server {
                listen_addr: "127.0.0.1:8080".parse().unwrap(),
                trusted_proxies: Vec::new(),
                keep_alive: true,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout: None,
                tls: None,
            },
            default_vhost: rginx_core::VirtualHost {
                server_names: Vec::new(),
                routes: Vec::new(),
                tls: None,
            },
            vhosts: Vec::new(),
            routes: Vec::new(),
            upstreams: HashMap::from([(name.to_string(), Arc::new(upstream))]),
        }
    }

    async fn spawn_status_server(statuses: Arc<Mutex<VecDeque<StatusCode>>>) -> SocketAddr {
        let listener =
            TcpListener::bind(("127.0.0.1", 0)).expect("test status listener should bind");
        let listen_addr = listener.local_addr().expect("listener addr should exist");

        thread::spawn(move || loop {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let statuses = statuses.clone();

            thread::spawn(move || {
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);
                let status = {
                    let mut statuses =
                        statuses.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    statuses.pop_front().unwrap_or(StatusCode::OK)
                };
                let reason = status.canonical_reason().unwrap_or("Unknown");
                let response = format!(
                    "HTTP/1.1 {} {}\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
                    status.as_u16(),
                    reason
                );

                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            });
        });

        listen_addr
    }

    const TEST_CA_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIDXTCCAkWgAwIBAgIJAOIvDiVb18eVMA0GCSqGSIb3DQEBCwUAMEUxCzAJBgNV\nBAYTAkFVMRMwEQYDVQQIDApTb21lLVN0YXRlMSEwHwYDVQQKDBhJbnRlcm5ldCBX\naWRnaXRzIFB0eSBMdGQwHhcNMTYwODE0MTY1NjExWhcNMjYwODEyMTY1NjExWjBF\nMQswCQYDVQQGEwJBVTETMBEGA1UECAwKU29tZS1TdGF0ZTEhMB8GA1UECgwYSW50\nZXJuZXQgV2lkZ2l0cyBQdHkgTHRkMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIB\nCgKCAQEArVHWFn52Lbl1l59exduZntVSZyDYpzDND+S2LUcO6fRBWhV/1Kzox+2G\nZptbuMGmfI3iAnb0CFT4uC3kBkQQlXonGATSVyaFTFR+jq/lc0SP+9Bd7SBXieIV\neIXlY1TvlwIvj3Ntw9zX+scTA4SXxH6M0rKv9gTOub2vCMSHeF16X8DQr4XsZuQr\n7Cp7j1I4aqOJyap5JTl5ijmG8cnu0n+8UcRlBzy99dLWJG0AfI3VRJdWpGTNVZ92\naFff3RpK3F/WI2gp3qV1ynRAKuvmncGC3LDvYfcc2dgsc1N6Ffq8GIrkgRob6eBc\nklDHp1d023Lwre+VaVDSo1//Y72UFwIDAQABo1AwTjAdBgNVHQ4EFgQUbNOlA6sN\nXyzJjYqciKeId7g3/ZowHwYDVR0jBBgwFoAUbNOlA6sNXyzJjYqciKeId7g3/Zow\nDAYDVR0TBAUwAwEB/zANBgkqhkiG9w0BAQsFAAOCAQEAVVaR5QWLZIRR4Dw6TSBn\nBQiLpBSXN6oAxdDw6n4PtwW6CzydaA+creiK6LfwEsiifUfQe9f+T+TBSpdIYtMv\nZ2H2tjlFX8VrjUFvPrvn5c28CuLI0foBgY8XGSkR2YMYzWw2jPEq3Th/KM5Catn3\nAFm3bGKWMtGPR4v+90chEN0jzaAmJYRrVUh9vea27bOCn31Nse6XXQPmSI6Gyncy\nOAPUsvPClF3IjeL1tmBotWqSGn1cYxLo+Lwjk22A9h6vjcNQRyZF2VLVvtwYrNU3\nmwJ6GCLsLHpwW/yjyvn8iEltnJvByM/eeRnfXV6WDObyiZsE/n6DxIRJodQzFqy9\nGA==\n-----END CERTIFICATE-----\n";
}
