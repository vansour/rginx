#[derive(Debug, Default)]
struct ResponseCounters {
    downstream_responses: AtomicU64,
    downstream_responses_1xx: AtomicU64,
    downstream_responses_2xx: AtomicU64,
    downstream_responses_3xx: AtomicU64,
    downstream_responses_4xx: AtomicU64,
    downstream_responses_5xx: AtomicU64,
}

#[derive(Debug, Default)]
struct RequestTrafficCounters {
    downstream_requests: AtomicU64,
    unmatched_requests_total: AtomicU64,
    responses: ResponseCounters,
    recent_60s: RecentTrafficStatsCounters,
    grpc: GrpcTrafficCounters,
}

#[derive(Debug, Default)]
pub(crate) struct ListenerTrafficCounters {
    downstream_connections_accepted: AtomicU64,
    downstream_connections_rejected: AtomicU64,
    active_http3_connections: AtomicUsize,
    active_http3_request_streams: AtomicUsize,
    http3_retry_issued_total: AtomicU64,
    http3_retry_failed_total: AtomicU64,
    http3_request_accept_errors_total: AtomicU64,
    http3_request_resolve_errors_total: AtomicU64,
    http3_request_body_stream_errors_total: AtomicU64,
    http3_response_stream_errors_total: AtomicU64,
    http3_connection_close_version_mismatch_total: AtomicU64,
    http3_connection_close_transport_error_total: AtomicU64,
    http3_connection_close_connection_closed_total: AtomicU64,
    http3_connection_close_application_closed_total: AtomicU64,
    http3_connection_close_reset_total: AtomicU64,
    http3_connection_close_timed_out_total: AtomicU64,
    http3_connection_close_locally_closed_total: AtomicU64,
    http3_connection_close_cids_exhausted_total: AtomicU64,
    downstream_mtls_authenticated_connections: AtomicU64,
    downstream_tls_handshake_failures: AtomicU64,
    downstream_tls_handshake_failures_missing_client_cert: AtomicU64,
    downstream_tls_handshake_failures_unknown_ca: AtomicU64,
    downstream_tls_handshake_failures_bad_certificate: AtomicU64,
    downstream_tls_handshake_failures_certificate_revoked: AtomicU64,
    downstream_tls_handshake_failures_verify_depth_exceeded: AtomicU64,
    downstream_tls_handshake_failures_other: AtomicU64,
    downstream_http3_early_data_accepted_requests: AtomicU64,
    downstream_http3_early_data_rejected_requests: AtomicU64,
    downstream_requests: AtomicU64,
    downstream_mtls_authenticated_requests: AtomicU64,
    downstream_mtls_anonymous_requests: AtomicU64,
    unmatched_requests_total: AtomicU64,
    responses: ResponseCounters,
    recent_60s: RecentTrafficStatsCounters,
    grpc: GrpcTrafficCounters,
}

#[derive(Debug, Default)]
struct RouteTrafficCounters {
    downstream_requests: AtomicU64,
    responses: ResponseCounters,
    access_denied_total: AtomicU64,
    rate_limited_total: AtomicU64,
    recent_60s: RecentTrafficStatsCounters,
    grpc: GrpcTrafficCounters,
}

#[derive(Debug)]
struct ListenerTrafficEntry {
    listener_name: String,
    listen_addr: std::net::SocketAddr,
    http3_enabled: bool,
    counters: Arc<ListenerTrafficCounters>,
}

#[derive(Debug)]
struct VhostTrafficEntry {
    server_names: Vec<String>,
    counters: Arc<RequestTrafficCounters>,
}

#[derive(Debug)]
struct RouteTrafficEntry {
    vhost_id: String,
    counters: Arc<RouteTrafficCounters>,
}

#[derive(Debug, Default)]
struct TrafficStatsIndex {
    listeners: HashMap<String, ListenerTrafficEntry>,
    listener_order: Vec<String>,
    vhosts: HashMap<String, VhostTrafficEntry>,
    vhost_order: Vec<String>,
    routes: HashMap<String, RouteTrafficEntry>,
    route_order: Vec<String>,
}

#[derive(Debug, Default)]
struct TrafficComponentVersions {
    listeners: HashMap<String, u64>,
    vhosts: HashMap<String, u64>,
    routes: HashMap<String, u64>,
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

fn build_traffic_stats_index(
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
                http3_enabled: listener.http3.is_some(),
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

fn build_traffic_component_versions(
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
