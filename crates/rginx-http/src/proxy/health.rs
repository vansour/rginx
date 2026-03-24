use super::clients::ProxyClients;
use super::*;

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
pub(super) struct PeerFailureStatus {
    pub consecutive_failures: u32,
    pub entered_cooldown: bool,
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
    active_requests: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ActiveProbeStatus {
    pub healthy: bool,
    pub recovered: bool,
    pub consecutive_successes: u32,
}

#[derive(Debug, Default)]
struct PeerHealth {
    state: Mutex<PeerHealthState>,
}

#[derive(Clone)]
pub(super) struct PeerHealthRegistry {
    policies: Arc<HashMap<String, PeerHealthPolicy>>,
    peers: Arc<HashMap<PeerHealthKey, Arc<PeerHealth>>>,
}

pub(super) struct SelectedPeers {
    pub peers: Vec<UpstreamPeer>,
    pub skipped_unhealthy: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PeerStatusSnapshot {
    pub url: String,
    pub weight: u32,
    pub backup: bool,
    pub healthy: bool,
    pub active_requests: u64,
    pub passive_consecutive_failures: u32,
    pub passive_cooldown_remaining_ms: Option<u64>,
    pub active_unhealthy: bool,
    pub active_consecutive_successes: u32,
}

impl PeerHealthPolicy {
    fn from_upstream(upstream: &Upstream) -> Self {
        Self {
            unhealthy_after_failures: upstream.unhealthy_after_failures,
            cooldown: upstream.unhealthy_cooldown,
        }
    }
}

impl PeerHealthRegistry {
    pub(super) fn from_config(config: &ConfigSnapshot) -> Self {
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

    pub(super) fn select_peers(
        &self,
        upstream: &Upstream,
        client_ip: std::net::IpAddr,
        limit: usize,
    ) -> SelectedPeers {
        if upstream.load_balance == UpstreamLoadBalance::LeastConn {
            return self.select_peers_by_least_conn(upstream, limit);
        }

        if !upstream.has_primary_peers() {
            return self.select_peers_in_pool(upstream, client_ip, limit, true);
        }

        let primary = self.select_peers_in_pool(upstream, client_ip, limit, false);
        if limit == 0 {
            return SelectedPeers { peers: Vec::new(), skipped_unhealthy: 0 };
        }

        if primary.peers.is_empty() {
            return merge_selected_peers(
                primary,
                self.select_peers_in_pool(upstream, client_ip, limit, true),
            );
        }

        if primary.peers.len() == limit {
            return primary;
        }

        let remaining = limit - primary.peers.len();
        merge_selected_peers(
            primary,
            self.select_peers_in_pool(upstream, client_ip, remaining, true),
        )
    }

    fn select_peers_by_least_conn(&self, upstream: &Upstream, limit: usize) -> SelectedPeers {
        if limit == 0 {
            return SelectedPeers { peers: Vec::new(), skipped_unhealthy: 0 };
        }

        if !upstream.has_primary_peers() {
            return self.select_peers_by_least_conn_in_pool(upstream, limit, true);
        }

        let primary = self.select_peers_by_least_conn_in_pool(upstream, limit, false);
        if primary.peers.is_empty() {
            return merge_selected_peers(
                primary,
                self.select_peers_by_least_conn_in_pool(upstream, limit, true),
            );
        }

        if primary.peers.len() == limit {
            return primary;
        }

        let remaining = limit - primary.peers.len();
        merge_selected_peers(
            primary,
            self.select_peers_by_least_conn_in_pool(upstream, remaining, true),
        )
    }

    fn select_peers_in_pool(
        &self,
        upstream: &Upstream,
        client_ip: std::net::IpAddr,
        limit: usize,
        backup: bool,
    ) -> SelectedPeers {
        let ordered = if backup {
            upstream.backup_peers_for_client_ip(client_ip, upstream.peers.len())
        } else {
            upstream.primary_peers_for_client_ip(client_ip, upstream.peers.len())
        };

        self.select_available_peers(upstream, ordered, limit)
    }

    fn select_peers_by_least_conn_in_pool(
        &self,
        upstream: &Upstream,
        limit: usize,
        backup: bool,
    ) -> SelectedPeers {
        let mut available = Vec::new();
        let mut skipped_unhealthy = 0;

        for (order, peer) in upstream.peers.iter().cloned().enumerate() {
            if peer.backup != backup {
                continue;
            }

            if self.is_available(&upstream.name, &peer.url) {
                available.push((self.active_requests(&upstream.name, &peer.url), order, peer));
            } else {
                skipped_unhealthy += 1;
            }
        }

        available.sort_by(|left, right| {
            projected_least_conn_load(left.0, left.2.weight, right.0, right.2.weight)
                .then(right.2.weight.cmp(&left.2.weight))
                .then(left.1.cmp(&right.1))
        });

        SelectedPeers {
            peers: available.into_iter().take(limit).map(|(_, _, peer)| peer).collect(),
            skipped_unhealthy,
        }
    }

    fn select_available_peers(
        &self,
        upstream: &Upstream,
        ordered: Vec<UpstreamPeer>,
        limit: usize,
    ) -> SelectedPeers {
        if limit == 0 {
            return SelectedPeers { peers: Vec::new(), skipped_unhealthy: 0 };
        }

        let mut selected = Vec::new();
        let mut skipped_unhealthy = 0;

        for peer in ordered {
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

    pub(super) fn record_success(&self, upstream_name: &str, peer_url: &str) {
        if let Some(health) = self.get(upstream_name, peer_url) {
            health.record_success();
        }
    }

    pub(super) fn record_failure(&self, upstream_name: &str, peer_url: &str) -> PeerFailureStatus {
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

    pub(super) fn record_active_success(
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

    pub(crate) fn record_active_failure(&self, upstream_name: &str, peer_url: &str) -> bool {
        self.get(upstream_name, peer_url).is_some_and(|health| health.record_active_failure())
    }

    pub(super) fn track_active_request(
        &self,
        upstream_name: &str,
        peer_url: &str,
    ) -> ActivePeerGuard {
        let peer = self.get(upstream_name, peer_url).cloned();
        if let Some(ref peer) = peer {
            peer.increment_active_requests();
        }

        ActivePeerGuard { peer }
    }

    fn active_requests(&self, upstream_name: &str, peer_url: &str) -> u64 {
        self.get(upstream_name, peer_url).map(|health| health.active_requests()).unwrap_or(0)
    }

    fn get(&self, upstream_name: &str, peer_url: &str) -> Option<&Arc<PeerHealth>> {
        self.peers.get(&PeerHealthKey {
            upstream_name: upstream_name.to_string(),
            peer_url: peer_url.to_string(),
        })
    }

    pub(super) fn snapshot(
        &self,
        upstream_name: &str,
        peer_url: &str,
        peer_display_url: &str,
        peer_weight: u32,
        peer_backup: bool,
    ) -> PeerStatusSnapshot {
        self.get(upstream_name, peer_url)
            .map(|health| health.snapshot(peer_display_url, peer_weight, peer_backup))
            .unwrap_or_else(|| PeerStatusSnapshot {
                url: peer_display_url.to_string(),
                weight: peer_weight,
                backup: peer_backup,
                healthy: true,
                active_requests: 0,
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

    fn increment_active_requests(&self) {
        lock_peer_health(&self.state).active_requests += 1;
    }

    fn decrement_active_requests(&self) {
        let mut state = lock_peer_health(&self.state);
        state.active_requests = state.active_requests.saturating_sub(1);
    }

    fn active_requests(&self) -> u64 {
        lock_peer_health(&self.state).active_requests
    }

    fn snapshot(&self, url: &str, weight: u32, backup: bool) -> PeerStatusSnapshot {
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
            weight,
            backup,
            healthy,
            active_requests: state.active_requests,
            passive_consecutive_failures: state.passive.consecutive_failures,
            passive_cooldown_remaining_ms,
            active_unhealthy: state.active.unhealthy,
            active_consecutive_successes: state.active.consecutive_successes,
        }
    }
}

pub(super) struct ActivePeerGuard {
    peer: Option<Arc<PeerHealth>>,
}

impl Drop for ActivePeerGuard {
    fn drop(&mut self) {
        if let Some(peer) = self.peer.take() {
            peer.decrement_active_requests();
        }
    }
}

pin_project! {
    pub(super) struct ActivePeerBody<B> {
        #[pin]
        inner: B,
        guard: Option<ActivePeerGuard>,
    }
}

impl<B> ActivePeerBody<B> {
    pub(super) fn new(inner: B, guard: ActivePeerGuard) -> Self {
        Self { inner, guard: Some(guard) }
    }
}

impl<B> hyper::body::Body for ActivePeerBody<B>
where
    B: hyper::body::Body<Data = Bytes>,
    B::Error: Into<BoxError>,
{
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();
        match this.inner.as_mut().poll_frame(cx) {
            std::task::Poll::Ready(Some(Ok(frame))) => std::task::Poll::Ready(Some(Ok(frame))),
            std::task::Poll::Ready(Some(Err(error))) => {
                this.guard.take();
                std::task::Poll::Ready(Some(Err(error.into())))
            }
            std::task::Poll::Ready(None) => {
                this.guard.take();
                std::task::Poll::Ready(None)
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

fn lock_peer_health(state: &Mutex<PeerHealthState>) -> std::sync::MutexGuard<'_, PeerHealthState> {
    state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn merge_selected_peers(mut primary: SelectedPeers, secondary: SelectedPeers) -> SelectedPeers {
    primary.skipped_unhealthy += secondary.skipped_unhealthy;
    primary.peers.extend(secondary.peers);
    primary
}

fn projected_least_conn_load(
    left_active_requests: u64,
    left_weight: u32,
    right_active_requests: u64,
    right_weight: u32,
) -> std::cmp::Ordering {
    let left = u128::from(left_active_requests.saturating_add(1)) * u128::from(right_weight.max(1));
    let right =
        u128::from(right_active_requests.saturating_add(1)) * u128::from(left_weight.max(1));
    left.cmp(&right)
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

    let request = match build_active_health_request(upstream.as_ref(), &peer, check) {
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
        Ok(Ok(response)) if check.grpc_service.is_some() => {
            match tokio::time::timeout(check.timeout, evaluate_grpc_health_probe_response(response))
                .await
            {
                Ok(Ok(GrpcHealthProbeResult::Serving)) => {
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
                            grpc_service = check.grpc_service.as_deref().unwrap_or(""),
                            consecutive_successes = status.consecutive_successes,
                            "active gRPC health check marked peer healthy"
                        );
                    }
                }
                Ok(Ok(GrpcHealthProbeResult::NotServing {
                    http_status,
                    grpc_status,
                    serving_status,
                })) => {
                    let transitioned =
                        clients.record_active_peer_failure(&upstream.name, &peer.url);
                    metrics.record_active_health_check(
                        &upstream.name,
                        &peer.url,
                        "unhealthy_status",
                    );
                    tracing::warn!(
                        upstream = %upstream.name,
                        peer = %peer.url,
                        path = %check.path,
                        grpc_service = check.grpc_service.as_deref().unwrap_or(""),
                        status = http_status.as_u16(),
                        grpc_status = grpc_status.as_deref().unwrap_or("-"),
                        serving_status = serving_status.map_or("-", grpc_health_serving_status_label),
                        state = if transitioned { "unhealthy" } else { "still unhealthy" },
                        "active gRPC health check returned an unhealthy status"
                    );
                }
                Ok(Err(error)) => {
                    let transitioned =
                        clients.record_active_peer_failure(&upstream.name, &peer.url);
                    metrics.record_active_health_check(&upstream.name, &peer.url, "request_error");
                    tracing::warn!(
                        upstream = %upstream.name,
                        peer = %peer.url,
                        path = %check.path,
                        grpc_service = check.grpc_service.as_deref().unwrap_or(""),
                        %error,
                        state = if transitioned { "unhealthy" } else { "still unhealthy" },
                        "active gRPC health check response could not be parsed"
                    );
                }
                Err(_) => {
                    let transitioned =
                        clients.record_active_peer_failure(&upstream.name, &peer.url);
                    metrics.record_active_health_check(&upstream.name, &peer.url, "timeout");
                    tracing::warn!(
                        upstream = %upstream.name,
                        peer = %peer.url,
                        path = %check.path,
                        grpc_service = check.grpc_service.as_deref().unwrap_or(""),
                        timeout_ms = check.timeout.as_millis() as u64,
                        state = if transitioned { "unhealthy" } else { "still unhealthy" },
                        "active gRPC health check timed out while reading response"
                    );
                }
            }
        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GrpcHealthServingStatus {
    Unknown,
    Serving,
    NotServing,
    ServiceUnknown,
    Other(u64),
}

impl GrpcHealthServingStatus {
    fn from_u64(value: u64) -> Self {
        match value {
            0 => Self::Unknown,
            GRPC_HEALTH_SERVING_STATUS_SERVING => Self::Serving,
            2 => Self::NotServing,
            3 => Self::ServiceUnknown,
            other => Self::Other(other),
        }
    }

    fn is_serving(self) -> bool {
        matches!(self, Self::Serving)
    }
}

#[derive(Debug)]
pub(super) enum GrpcHealthProbeResult {
    Serving,
    NotServing {
        http_status: StatusCode,
        grpc_status: Option<String>,
        serving_status: Option<GrpcHealthServingStatus>,
    },
}

fn grpc_health_serving_status_label(status: GrpcHealthServingStatus) -> &'static str {
    match status {
        GrpcHealthServingStatus::Unknown => "UNKNOWN",
        GrpcHealthServingStatus::Serving => "SERVING",
        GrpcHealthServingStatus::NotServing => "NOT_SERVING",
        GrpcHealthServingStatus::ServiceUnknown => "SERVICE_UNKNOWN",
        GrpcHealthServingStatus::Other(_) => "OTHER",
    }
}

pub(super) async fn evaluate_grpc_health_probe_response<B>(
    response: Response<B>,
) -> Result<GrpcHealthProbeResult, BoxError>
where
    B: hyper::body::Body<Data = Bytes> + Unpin,
    B::Error: Into<BoxError>,
{
    let (parts, body) = response.into_parts();
    let response_headers = parts.headers;
    let http_status = parts.status;
    let (body, trailers) = collect_response_body_and_trailers(body).await?;
    let grpc_status = grpc_trailer_value(&response_headers, trailers.as_ref(), "grpc-status");
    let serving_status = decode_grpc_health_check_response(body.as_ref())?;

    Ok(
        if http_status.is_success()
            && grpc_status.as_deref() == Some("0")
            && serving_status.is_some_and(GrpcHealthServingStatus::is_serving)
        {
            GrpcHealthProbeResult::Serving
        } else {
            GrpcHealthProbeResult::NotServing { http_status, grpc_status, serving_status }
        },
    )
}

async fn collect_response_body_and_trailers<B>(
    mut body: B,
) -> Result<(Bytes, Option<HeaderMap>), BoxError>
where
    B: hyper::body::Body<Data = Bytes> + Unpin,
    B::Error: Into<BoxError>,
{
    let mut collected = BytesMut::new();
    let mut trailers = None;

    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(Into::<BoxError>::into)?;
        let frame = match frame.into_data() {
            Ok(data) => {
                collected.extend_from_slice(&data);
                continue;
            }
            Err(frame) => frame,
        };

        let frame_trailers = match frame.into_trailers() {
            Ok(trailers) => trailers,
            Err(_) => continue,
        };
        if let Some(existing) = trailers.as_mut() {
            append_header_map(existing, &frame_trailers);
        } else {
            trailers = Some(frame_trailers);
        }
    }

    Ok((collected.freeze(), trailers))
}

fn grpc_trailer_value(
    headers: &HeaderMap,
    trailers: Option<&HeaderMap>,
    name: &str,
) -> Option<String> {
    let name = HeaderName::from_bytes(name.as_bytes()).ok()?;
    trailers
        .and_then(|trailers| trailers.get(&name))
        .or_else(|| headers.get(&name))
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

pub(super) fn encode_grpc_health_check_request(service: &str) -> Bytes {
    let mut payload = BytesMut::new();
    if !service.is_empty() {
        payload.extend_from_slice(&[0x0a]);
        append_protobuf_varint(&mut payload, service.len() as u64);
        payload.extend_from_slice(service.as_bytes());
    }

    let mut frame = BytesMut::with_capacity(5 + payload.len());
    frame.extend_from_slice(&[0]);
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(&payload);
    frame.freeze()
}

pub(super) fn decode_grpc_health_check_response(
    body: &[u8],
) -> Result<Option<GrpcHealthServingStatus>, BoxError> {
    if body.is_empty() {
        return Ok(None);
    }

    let payload = decode_grpc_frame_payload(body)?;
    Ok(Some(decode_grpc_health_check_response_payload(payload)?))
}

fn decode_grpc_frame_payload(body: &[u8]) -> Result<&[u8], BoxError> {
    if body.len() < 5 {
        return Err(invalid_grpc_health_probe("incomplete gRPC health response frame header"));
    }

    let compressed = body[0];
    if compressed != 0 {
        return Err(invalid_grpc_health_probe(
            "compressed gRPC health responses are not supported",
        ));
    }

    let len = u32::from_be_bytes([body[1], body[2], body[3], body[4]]) as usize;
    let expected_len = 5 + len;
    if body.len() != expected_len {
        return Err(invalid_grpc_health_probe(format!(
            "gRPC health response frame length mismatch: expected {expected_len} bytes, got {}",
            body.len()
        )));
    }

    Ok(&body[5..])
}

fn decode_grpc_health_check_response_payload(
    payload: &[u8],
) -> Result<GrpcHealthServingStatus, BoxError> {
    let mut index = 0usize;
    let mut serving_status = GrpcHealthServingStatus::Unknown;

    while index < payload.len() {
        let tag = decode_protobuf_varint(payload, &mut index)?;
        let field_number = tag >> 3;
        let wire_type = (tag & 0x07) as u8;

        match (field_number, wire_type) {
            (1, 0) => {
                serving_status =
                    GrpcHealthServingStatus::from_u64(decode_protobuf_varint(payload, &mut index)?);
            }
            _ => skip_protobuf_field(payload, &mut index, wire_type)?,
        }
    }

    Ok(serving_status)
}

fn append_protobuf_varint(buffer: &mut BytesMut, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buffer.extend_from_slice(&[byte]);
        if value == 0 {
            break;
        }
    }
}

fn decode_protobuf_varint(payload: &[u8], index: &mut usize) -> Result<u64, BoxError> {
    let mut value = 0u64;
    let mut shift = 0u32;

    while *index < payload.len() {
        let byte = payload[*index];
        *index += 1;
        value |= u64::from(byte & 0x7f) << shift;

        if byte & 0x80 == 0 {
            return Ok(value);
        }

        shift += 7;
        if shift >= 64 {
            return Err(invalid_grpc_health_probe("protobuf varint is too large"));
        }
    }

    Err(invalid_grpc_health_probe("unexpected EOF while decoding protobuf varint"))
}

fn skip_protobuf_field(payload: &[u8], index: &mut usize, wire_type: u8) -> Result<(), BoxError> {
    match wire_type {
        0 => {
            let _ = decode_protobuf_varint(payload, index)?;
        }
        1 => {
            let end = index.saturating_add(8);
            if end > payload.len() {
                return Err(invalid_grpc_health_probe(
                    "unexpected EOF while skipping fixed64 protobuf field",
                ));
            }
            *index = end;
        }
        2 => {
            let len = usize::try_from(decode_protobuf_varint(payload, index)?).map_err(|_| {
                invalid_grpc_health_probe("length-delimited protobuf field exceeds platform limits")
            })?;
            let end = index.saturating_add(len);
            if end > payload.len() {
                return Err(invalid_grpc_health_probe(
                    "unexpected EOF while skipping length-delimited protobuf field",
                ));
            }
            *index = end;
        }
        5 => {
            let end = index.saturating_add(4);
            if end > payload.len() {
                return Err(invalid_grpc_health_probe(
                    "unexpected EOF while skipping fixed32 protobuf field",
                ));
            }
            *index = end;
        }
        _ => {
            return Err(invalid_grpc_health_probe(format!(
                "unsupported protobuf wire type `{wire_type}` in gRPC health response"
            )));
        }
    }

    Ok(())
}

fn invalid_grpc_health_probe(message: impl Into<String>) -> BoxError {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into()).into()
}

pub(super) fn build_active_health_request(
    upstream: &Upstream,
    peer: &UpstreamPeer,
    check: &ActiveHealthCheck,
) -> Result<Request<HttpBody>, Error> {
    let path = &check.path;
    let path: Uri = path.parse().map_err(|error| {
        Error::Server(format!(
            "failed to parse active health-check path `{path}` for peer `{}`: {error}",
            peer.url
        ))
    })?;
    let uri = build_proxy_uri(peer, &path, None).map_err(|error| {
        Error::Server(format!("failed to build active health-check uri: {error}"))
    })?;

    let mut builder = Request::builder().uri(uri).header(HOST, peer.authority.as_str());

    if let Some(service) = check.grpc_service.as_deref() {
        let body = encode_grpc_health_check_request(service);
        builder = builder
            .method(Method::POST)
            .version(Version::HTTP_2)
            .header(CONTENT_TYPE, HeaderValue::from_static("application/grpc"))
            .header(TE, HeaderValue::from_static("trailers"))
            .header(CONTENT_LENGTH, body.len().to_string());
        builder.body(full_body(body)).map_err(|error| {
            Error::Server(format!("failed to build active health-check request: {error}"))
        })
    } else {
        builder
            .method(Method::GET)
            .version(upstream_request_version(upstream.protocol))
            .body(full_body(Bytes::new()))
            .map_err(|error| {
                Error::Server(format!("failed to build active health-check request: {error}"))
            })
    }
}
