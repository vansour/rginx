#[derive(Debug, Default)]
pub(super) struct HttpCounters {
    downstream_connections_accepted: AtomicU64,
    downstream_connections_rejected: AtomicU64,
    downstream_requests: AtomicU64,
    downstream_responses: AtomicU64,
    downstream_responses_1xx: AtomicU64,
    downstream_responses_2xx: AtomicU64,
    downstream_responses_3xx: AtomicU64,
    downstream_responses_4xx: AtomicU64,
    downstream_responses_5xx: AtomicU64,
    downstream_mtls_authenticated_connections: AtomicU64,
    downstream_mtls_authenticated_requests: AtomicU64,
    downstream_mtls_anonymous_requests: AtomicU64,
    downstream_tls_handshake_failures: AtomicU64,
    downstream_tls_handshake_failures_missing_client_cert: AtomicU64,
    downstream_tls_handshake_failures_unknown_ca: AtomicU64,
    downstream_tls_handshake_failures_bad_certificate: AtomicU64,
    downstream_tls_handshake_failures_certificate_revoked: AtomicU64,
    downstream_tls_handshake_failures_verify_depth_exceeded: AtomicU64,
    downstream_tls_handshake_failures_other: AtomicU64,
}

#[derive(Debug, Default)]
pub(super) struct ReloadHistory {
    attempts_total: u64,
    successes_total: u64,
    failures_total: u64,
    last_result: Option<ReloadResultSnapshot>,
}

#[derive(Debug, Default)]
pub(super) struct UpstreamStats {
    downstream_requests_total: AtomicU64,
    peer_attempts_total: AtomicU64,
    peer_successes_total: AtomicU64,
    peer_failures_total: AtomicU64,
    peer_timeouts_total: AtomicU64,
    failovers_total: AtomicU64,
    completed_responses_total: AtomicU64,
    bad_gateway_responses_total: AtomicU64,
    gateway_timeout_responses_total: AtomicU64,
    bad_request_responses_total: AtomicU64,
    payload_too_large_responses_total: AtomicU64,
    unsupported_media_type_responses_total: AtomicU64,
    no_healthy_peers_total: AtomicU64,
    tls_failures_unknown_ca_total: AtomicU64,
    tls_failures_bad_certificate_total: AtomicU64,
    tls_failures_certificate_revoked_total: AtomicU64,
    tls_failures_verify_depth_exceeded_total: AtomicU64,
    recent_60s: RecentUpstreamStatsCounters,
}

#[derive(Debug, Default)]
pub(super) struct UpstreamPeerStats {
    attempts_total: AtomicU64,
    successes_total: AtomicU64,
    failures_total: AtomicU64,
    timeouts_total: AtomicU64,
}

#[derive(Debug)]
pub(super) struct UpstreamStatsEntry {
    upstream: Arc<rginx_core::Upstream>,
    counters: Arc<UpstreamStats>,
    peers: HashMap<String, Arc<UpstreamPeerStats>>,
    peer_order: Vec<String>,
}

#[derive(Debug, Default)]
pub(super) struct ResponseCounters {
    downstream_responses: AtomicU64,
    downstream_responses_1xx: AtomicU64,
    downstream_responses_2xx: AtomicU64,
    downstream_responses_3xx: AtomicU64,
    downstream_responses_4xx: AtomicU64,
    downstream_responses_5xx: AtomicU64,
}

#[derive(Debug, Default)]
pub(super) struct RollingCounter {
    buckets: Mutex<VecDeque<(u64, u64)>>,
}

#[derive(Debug, Default)]
pub(super) struct RecentTrafficStatsCounters {
    downstream_requests_total: RollingCounter,
    downstream_responses_total: RollingCounter,
    downstream_responses_2xx_total: RollingCounter,
    downstream_responses_4xx_total: RollingCounter,
    downstream_responses_5xx_total: RollingCounter,
    grpc_requests_total: RollingCounter,
}

#[derive(Debug, Default)]
pub(super) struct RecentUpstreamStatsCounters {
    downstream_requests_total: RollingCounter,
    peer_attempts_total: RollingCounter,
    completed_responses_total: RollingCounter,
    bad_gateway_responses_total: RollingCounter,
    gateway_timeout_responses_total: RollingCounter,
    failovers_total: RollingCounter,
}

#[derive(Debug, Default)]
pub(super) struct GrpcTrafficCounters {
    requests_total: AtomicU64,
    protocol_grpc_total: AtomicU64,
    protocol_grpc_web_total: AtomicU64,
    protocol_grpc_web_text_total: AtomicU64,
    status_0_total: AtomicU64,
    status_1_total: AtomicU64,
    status_3_total: AtomicU64,
    status_4_total: AtomicU64,
    status_7_total: AtomicU64,
    status_8_total: AtomicU64,
    status_12_total: AtomicU64,
    status_14_total: AtomicU64,
    status_other_total: AtomicU64,
}

#[derive(Debug, Default)]
pub(super) struct RequestTrafficCounters {
    downstream_requests: AtomicU64,
    unmatched_requests_total: AtomicU64,
    responses: ResponseCounters,
    recent_60s: RecentTrafficStatsCounters,
    grpc: GrpcTrafficCounters,
}

#[derive(Debug, Default)]
pub(super) struct ListenerTrafficCounters {
    downstream_connections_accepted: AtomicU64,
    downstream_connections_rejected: AtomicU64,
    downstream_mtls_authenticated_connections: AtomicU64,
    downstream_tls_handshake_failures: AtomicU64,
    downstream_tls_handshake_failures_missing_client_cert: AtomicU64,
    downstream_tls_handshake_failures_unknown_ca: AtomicU64,
    downstream_tls_handshake_failures_bad_certificate: AtomicU64,
    downstream_tls_handshake_failures_certificate_revoked: AtomicU64,
    downstream_tls_handshake_failures_verify_depth_exceeded: AtomicU64,
    downstream_tls_handshake_failures_other: AtomicU64,
    downstream_requests: AtomicU64,
    downstream_mtls_authenticated_requests: AtomicU64,
    downstream_mtls_anonymous_requests: AtomicU64,
    unmatched_requests_total: AtomicU64,
    responses: ResponseCounters,
    recent_60s: RecentTrafficStatsCounters,
    grpc: GrpcTrafficCounters,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TlsHandshakeFailureReason {
    MissingClientCert,
    UnknownCa,
    BadCertificate,
    CertificateRevoked,
    VerifyDepthExceeded,
    Other,
}

impl TlsHandshakeFailureReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::MissingClientCert => "missing_client_cert",
            Self::UnknownCa => "unknown_ca",
            Self::BadCertificate => "bad_certificate",
            Self::CertificateRevoked => "certificate_revoked",
            Self::VerifyDepthExceeded => "verify_depth_exceeded",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct RouteTrafficCounters {
    downstream_requests: AtomicU64,
    responses: ResponseCounters,
    access_denied_total: AtomicU64,
    rate_limited_total: AtomicU64,
    recent_60s: RecentTrafficStatsCounters,
    grpc: GrpcTrafficCounters,
}

#[derive(Debug)]
pub(super) struct ListenerTrafficEntry {
    listener_name: String,
    listen_addr: std::net::SocketAddr,
    counters: Arc<ListenerTrafficCounters>,
}

#[derive(Debug)]
pub(super) struct VhostTrafficEntry {
    server_names: Vec<String>,
    counters: Arc<RequestTrafficCounters>,
}

#[derive(Debug)]
pub(super) struct RouteTrafficEntry {
    vhost_id: String,
    counters: Arc<RouteTrafficCounters>,
}

#[derive(Debug, Default)]
pub(super) struct TrafficStatsIndex {
    listeners: HashMap<String, ListenerTrafficEntry>,
    listener_order: Vec<String>,
    vhosts: HashMap<String, VhostTrafficEntry>,
    vhost_order: Vec<String>,
    routes: HashMap<String, RouteTrafficEntry>,
    route_order: Vec<String>,
}

#[derive(Debug, Default)]
pub(super) struct SnapshotComponentVersions {
    status: AtomicU64,
    counters: AtomicU64,
    traffic: AtomicU64,
    peer_health: AtomicU64,
    upstreams: AtomicU64,
}

#[derive(Debug, Default)]
pub(super) struct TrafficComponentVersions {
    listeners: HashMap<String, u64>,
    vhosts: HashMap<String, u64>,
    routes: HashMap<String, u64>,
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
            downstream_mtls_authenticated_connections: self
                .downstream_mtls_authenticated_connections
                .load(Ordering::Relaxed),
            downstream_mtls_authenticated_requests: self
                .downstream_mtls_authenticated_requests
                .load(Ordering::Relaxed),
            downstream_mtls_anonymous_requests: self
                .downstream_mtls_anonymous_requests
                .load(Ordering::Relaxed),
            downstream_tls_handshake_failures: self
                .downstream_tls_handshake_failures
                .load(Ordering::Relaxed),
            downstream_tls_handshake_failures_missing_client_cert: self
                .downstream_tls_handshake_failures_missing_client_cert
                .load(Ordering::Relaxed),
            downstream_tls_handshake_failures_unknown_ca: self
                .downstream_tls_handshake_failures_unknown_ca
                .load(Ordering::Relaxed),
            downstream_tls_handshake_failures_bad_certificate: self
                .downstream_tls_handshake_failures_bad_certificate
                .load(Ordering::Relaxed),
            downstream_tls_handshake_failures_certificate_revoked: self
                .downstream_tls_handshake_failures_certificate_revoked
                .load(Ordering::Relaxed),
            downstream_tls_handshake_failures_verify_depth_exceeded: self
                .downstream_tls_handshake_failures_verify_depth_exceeded
                .load(Ordering::Relaxed),
            downstream_tls_handshake_failures_other: self
                .downstream_tls_handshake_failures_other
                .load(Ordering::Relaxed),
        }
    }
}

impl ResponseCounters {
    fn record(&self, status: StatusCode) {
        self.downstream_responses.fetch_add(1, Ordering::Relaxed);
        match status.as_u16() / 100 {
            1 => {
                self.downstream_responses_1xx.fetch_add(1, Ordering::Relaxed);
            }
            2 => {
                self.downstream_responses_2xx.fetch_add(1, Ordering::Relaxed);
            }
            3 => {
                self.downstream_responses_3xx.fetch_add(1, Ordering::Relaxed);
            }
            4 => {
                self.downstream_responses_4xx.fetch_add(1, Ordering::Relaxed);
            }
            5 => {
                self.downstream_responses_5xx.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }

    fn snapshot(&self) -> (u64, u64, u64, u64, u64, u64) {
        (
            self.downstream_responses.load(Ordering::Relaxed),
            self.downstream_responses_1xx.load(Ordering::Relaxed),
            self.downstream_responses_2xx.load(Ordering::Relaxed),
            self.downstream_responses_3xx.load(Ordering::Relaxed),
            self.downstream_responses_4xx.load(Ordering::Relaxed),
            self.downstream_responses_5xx.load(Ordering::Relaxed),
        )
    }
}

impl RollingCounter {
    fn increment_now(&self) {
        self.increment_at(window_now_secs());
    }

    fn increment_at(&self, second: u64) {
        let mut buckets = self.buckets.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        trim_old_buckets(&mut buckets, second, MAX_RECENT_WINDOW_SECS);
        match buckets.back_mut() {
            Some((bucket_second, count)) if *bucket_second == second => {
                *count += 1;
            }
            _ => buckets.push_back((second, 1)),
        }
    }

    fn sum_recent(&self, now_second: u64, window_secs: u64) -> u64 {
        let mut buckets = self.buckets.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        trim_old_buckets(&mut buckets, now_second, MAX_RECENT_WINDOW_SECS);
        let cutoff = now_second.saturating_sub(window_secs.saturating_sub(1));
        buckets.iter().filter_map(|(second, count)| (*second >= cutoff).then_some(*count)).sum()
    }
}

impl RecentTrafficStatsCounters {
    fn snapshot(&self) -> RecentTrafficStatsSnapshot {
        self.snapshot_for_window(RECENT_WINDOW_SECS)
    }

    fn snapshot_for_window(&self, window_secs: u64) -> RecentTrafficStatsSnapshot {
        let now = window_now_secs();
        RecentTrafficStatsSnapshot {
            window_secs,
            downstream_requests_total: self.downstream_requests_total.sum_recent(now, window_secs),
            downstream_responses_total: self
                .downstream_responses_total
                .sum_recent(now, window_secs),
            downstream_responses_2xx_total: self
                .downstream_responses_2xx_total
                .sum_recent(now, window_secs),
            downstream_responses_4xx_total: self
                .downstream_responses_4xx_total
                .sum_recent(now, window_secs),
            downstream_responses_5xx_total: self
                .downstream_responses_5xx_total
                .sum_recent(now, window_secs),
            grpc_requests_total: self.grpc_requests_total.sum_recent(now, window_secs),
        }
    }

    fn record_downstream_request(&self) {
        self.downstream_requests_total.increment_now();
    }

    fn record_downstream_response(&self, status: StatusCode) {
        self.downstream_responses_total.increment_now();
        match status.as_u16() / 100 {
            2 => self.downstream_responses_2xx_total.increment_now(),
            4 => self.downstream_responses_4xx_total.increment_now(),
            5 => self.downstream_responses_5xx_total.increment_now(),
            _ => {}
        }
    }

    fn record_grpc_request(&self) {
        self.grpc_requests_total.increment_now();
    }
}

impl RecentUpstreamStatsCounters {
    fn snapshot(&self) -> RecentUpstreamStatsSnapshot {
        self.snapshot_for_window(RECENT_WINDOW_SECS)
    }

    fn snapshot_for_window(&self, window_secs: u64) -> RecentUpstreamStatsSnapshot {
        let now = window_now_secs();
        RecentUpstreamStatsSnapshot {
            window_secs,
            downstream_requests_total: self.downstream_requests_total.sum_recent(now, window_secs),
            peer_attempts_total: self.peer_attempts_total.sum_recent(now, window_secs),
            completed_responses_total: self.completed_responses_total.sum_recent(now, window_secs),
            bad_gateway_responses_total: self
                .bad_gateway_responses_total
                .sum_recent(now, window_secs),
            gateway_timeout_responses_total: self
                .gateway_timeout_responses_total
                .sum_recent(now, window_secs),
            failovers_total: self.failovers_total.sum_recent(now, window_secs),
        }
    }
}

impl GrpcTrafficCounters {
    fn record_request(&self, protocol: &str) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        match protocol {
            "grpc" => {
                self.protocol_grpc_total.fetch_add(1, Ordering::Relaxed);
            }
            "grpc-web" => {
                self.protocol_grpc_web_total.fetch_add(1, Ordering::Relaxed);
            }
            "grpc-web-text" => {
                self.protocol_grpc_web_text_total.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }

    fn record_status(&self, status: Option<&str>) {
        match status {
            Some("0") => {
                self.status_0_total.fetch_add(1, Ordering::Relaxed);
            }
            Some("1") => {
                self.status_1_total.fetch_add(1, Ordering::Relaxed);
            }
            Some("3") => {
                self.status_3_total.fetch_add(1, Ordering::Relaxed);
            }
            Some("4") => {
                self.status_4_total.fetch_add(1, Ordering::Relaxed);
            }
            Some("7") => {
                self.status_7_total.fetch_add(1, Ordering::Relaxed);
            }
            Some("8") => {
                self.status_8_total.fetch_add(1, Ordering::Relaxed);
            }
            Some("12") => {
                self.status_12_total.fetch_add(1, Ordering::Relaxed);
            }
            Some("14") => {
                self.status_14_total.fetch_add(1, Ordering::Relaxed);
            }
            _ => {
                self.status_other_total.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn snapshot(&self) -> GrpcTrafficSnapshot {
        GrpcTrafficSnapshot {
            requests_total: self.requests_total.load(Ordering::Relaxed),
            protocol_grpc_total: self.protocol_grpc_total.load(Ordering::Relaxed),
            protocol_grpc_web_total: self.protocol_grpc_web_total.load(Ordering::Relaxed),
            protocol_grpc_web_text_total: self.protocol_grpc_web_text_total.load(Ordering::Relaxed),
            status_0_total: self.status_0_total.load(Ordering::Relaxed),
            status_1_total: self.status_1_total.load(Ordering::Relaxed),
            status_3_total: self.status_3_total.load(Ordering::Relaxed),
            status_4_total: self.status_4_total.load(Ordering::Relaxed),
            status_7_total: self.status_7_total.load(Ordering::Relaxed),
            status_8_total: self.status_8_total.load(Ordering::Relaxed),
            status_12_total: self.status_12_total.load(Ordering::Relaxed),
            status_14_total: self.status_14_total.load(Ordering::Relaxed),
            status_other_total: self.status_other_total.load(Ordering::Relaxed),
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

pub(super) fn build_traffic_stats_index(
    config: &ConfigSnapshot,
    existing: Option<&TrafficStatsIndex>,
) -> TrafficStatsIndex {
    let mut index = TrafficStatsIndex::default();

    for listener in &config.listeners {
        let current = existing.and_then(|stats| stats.listeners.get(&listener.id));
        index.listener_order.push(listener.id.clone());
        index.listeners.insert(
            listener.id.clone(),
            ListenerTrafficEntry {
                listener_name: listener.name.clone(),
                listen_addr: listener.server.listen_addr,
                counters: current
                    .map(|entry| entry.counters.clone())
                    .unwrap_or_else(|| Arc::new(ListenerTrafficCounters::default())),
            },
        );
    }

    let mut register_vhost = |vhost: &rginx_core::VirtualHost| {
        let current = existing.and_then(|stats| stats.vhosts.get(&vhost.id));
        index.vhost_order.push(vhost.id.clone());
        index.vhosts.insert(
            vhost.id.clone(),
            VhostTrafficEntry {
                server_names: vhost.server_names.clone(),
                counters: current
                    .map(|entry| entry.counters.clone())
                    .unwrap_or_else(|| Arc::new(RequestTrafficCounters::default())),
            },
        );

        for route in &vhost.routes {
            let current = existing.and_then(|stats| stats.routes.get(&route.id));
            index.route_order.push(route.id.clone());
            index.routes.insert(
                route.id.clone(),
                RouteTrafficEntry {
                    vhost_id: vhost.id.clone(),
                    counters: current
                        .map(|entry| entry.counters.clone())
                        .unwrap_or_else(|| Arc::new(RouteTrafficCounters::default())),
                },
            );
        }
    };

    register_vhost(&config.default_vhost);
    for vhost in &config.vhosts {
        register_vhost(vhost);
    }

    index
}

pub(super) fn build_traffic_component_versions(
    config: &ConfigSnapshot,
    existing: Option<&TrafficComponentVersions>,
) -> TrafficComponentVersions {
    let mut versions = TrafficComponentVersions::default();

    for listener in &config.listeners {
        let version =
            existing.and_then(|current| current.listeners.get(&listener.id)).copied().unwrap_or(0);
        versions.listeners.insert(listener.id.clone(), version);
    }

    let mut register_vhost = |vhost: &rginx_core::VirtualHost| {
        let version =
            existing.and_then(|current| current.vhosts.get(&vhost.id)).copied().unwrap_or(0);
        versions.vhosts.insert(vhost.id.clone(), version);

        for route in &vhost.routes {
            let version =
                existing.and_then(|current| current.routes.get(&route.id)).copied().unwrap_or(0);
            versions.routes.insert(route.id.clone(), version);
        }
    };

    register_vhost(&config.default_vhost);
    for vhost in &config.vhosts {
        register_vhost(vhost);
    }

    versions
}

pub(super) fn build_upstream_stats_map(
    config: &ConfigSnapshot,
    existing: Option<&HashMap<String, UpstreamStatsEntry>>,
) -> HashMap<String, UpstreamStatsEntry> {
    config
        .upstreams
        .values()
        .map(|upstream| {
            let current = existing.and_then(|stats| stats.get(&upstream.name));
            let peers = upstream
                .peers
                .iter()
                .map(|peer| {
                    let stats = current
                        .and_then(|entry| entry.peers.get(&peer.url))
                        .cloned()
                        .unwrap_or_else(|| Arc::new(UpstreamPeerStats::default()));
                    (peer.url.clone(), stats)
                })
                .collect::<HashMap<_, _>>();

            (
                upstream.name.clone(),
                UpstreamStatsEntry {
                    upstream: upstream.clone(),
                    counters: current
                        .map(|entry| entry.counters.clone())
                        .unwrap_or_else(|| Arc::new(UpstreamStats::default())),
                    peers,
                    peer_order: upstream.peers.iter().map(|peer| peer.url.clone()).collect(),
                },
            )
        })
        .collect()
}

pub(super) fn build_upstream_name_versions(
    config: &ConfigSnapshot,
    existing: Option<&HashMap<String, u64>>,
) -> HashMap<String, u64> {
    config
        .upstreams
        .keys()
        .map(|name| {
            let version = existing.and_then(|current| current.get(name)).copied().unwrap_or(0);
            (name.clone(), version)
        })
        .collect()
}

pub(super) fn window_now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|duration| duration.as_secs()).unwrap_or(0)
}

pub(super) fn trim_old_buckets(buckets: &mut VecDeque<(u64, u64)>, now_second: u64, window_secs: u64) {
    let cutoff = now_second.saturating_sub(window_secs.saturating_sub(1));
    while buckets.front().is_some_and(|(second, _)| *second < cutoff) {
        buckets.pop_front();
    }
}
