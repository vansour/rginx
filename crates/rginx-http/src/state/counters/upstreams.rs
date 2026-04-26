#[derive(Debug, Default)]
struct UpstreamStats {
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
struct UpstreamPeerStats {
    attempts_total: AtomicU64,
    successes_total: AtomicU64,
    failures_total: AtomicU64,
    timeouts_total: AtomicU64,
}

#[derive(Debug)]
struct UpstreamStatsEntry {
    upstream: Arc<rginx_core::Upstream>,
    counters: Arc<UpstreamStats>,
    peers: HashMap<String, Arc<UpstreamPeerStats>>,
    peer_order: Vec<String>,
}

fn build_upstream_stats_map(
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

fn build_upstream_name_versions(
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
