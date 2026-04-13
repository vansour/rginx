pub(super) fn prepare_state(
    config: ConfigSnapshot,
    peer_health_notifier: Option<HealthChangeNotifier>,
) -> Result<PreparedState> {
    let config = Arc::new(config);
    let clients =
        ProxyClients::from_config_with_health_notifier(config.as_ref(), peer_health_notifier)?;
    let listener_tls_acceptors = prepare_listener_tls_acceptors(config.as_ref())?;

    Ok(PreparedState {
        config,
        clients,
        listener_tls_acceptors,
        retired_listeners: Vec::new(),
    })
}

pub(super) fn prepare_listener_tls_acceptors(
    config: &ConfigSnapshot,
) -> Result<HashMap<String, Option<TlsAcceptor>>> {
    config
        .listeners
        .iter()
        .map(|listener| {
            let tls_acceptor = build_tls_acceptor(
                listener.server.tls.as_ref(),
                listener.server.default_certificate.as_deref(),
                listener.tls_enabled(),
                &config.default_vhost,
                &config.vhosts,
            )?;
            Ok((listener.id.clone(), tls_acceptor))
        })
        .collect::<Result<HashMap<_, _>>>()
}

pub(super) fn build_peer_health_notifier(
    snapshot_version: Arc<AtomicU64>,
    snapshot_notify: Arc<Notify>,
    snapshot_components: Arc<SnapshotComponentVersions>,
    peer_health_component_versions: Arc<StdRwLock<HashMap<String, u64>>>,
) -> HealthChangeNotifier {
    Arc::new(move |upstream_name| {
        let version = snapshot_version.fetch_add(1, Ordering::Relaxed) + 1;
        snapshot_components.peer_health.store(version, Ordering::Relaxed);
        peer_health_component_versions
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(upstream_name.to_string(), version);
        snapshot_notify.notify_waiters();
    })
}

pub(super) fn unix_time_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

pub(super) fn http3_runtime_snapshot(
    http3_enabled: bool,
    counters: Option<&ListenerTrafficCounters>,
) -> Option<Http3ListenerRuntimeSnapshot> {
    if !http3_enabled {
        return None;
    }
    let counters = counters?;
    Some(Http3ListenerRuntimeSnapshot {
        active_connections: counters.active_http3_connections.load(Ordering::Acquire),
        active_request_streams: counters.active_http3_request_streams.load(Ordering::Acquire),
        retry_issued_total: counters.http3_retry_issued_total.load(Ordering::Relaxed),
        retry_failed_total: counters.http3_retry_failed_total.load(Ordering::Relaxed),
        request_accept_errors_total: counters
            .http3_request_accept_errors_total
            .load(Ordering::Relaxed),
        request_resolve_errors_total: counters
            .http3_request_resolve_errors_total
            .load(Ordering::Relaxed),
        request_body_stream_errors_total: counters
            .http3_request_body_stream_errors_total
            .load(Ordering::Relaxed),
        response_stream_errors_total: counters
            .http3_response_stream_errors_total
            .load(Ordering::Relaxed),
        connection_close_version_mismatch_total: counters
            .http3_connection_close_version_mismatch_total
            .load(Ordering::Relaxed),
        connection_close_transport_error_total: counters
            .http3_connection_close_transport_error_total
            .load(Ordering::Relaxed),
        connection_close_connection_closed_total: counters
            .http3_connection_close_connection_closed_total
            .load(Ordering::Relaxed),
        connection_close_application_closed_total: counters
            .http3_connection_close_application_closed_total
            .load(Ordering::Relaxed),
        connection_close_reset_total: counters
            .http3_connection_close_reset_total
            .load(Ordering::Relaxed),
        connection_close_timed_out_total: counters
            .http3_connection_close_timed_out_total
            .load(Ordering::Relaxed),
        connection_close_locally_closed_total: counters
            .http3_connection_close_locally_closed_total
            .load(Ordering::Relaxed),
        connection_close_cids_exhausted_total: counters
            .http3_connection_close_cids_exhausted_total
            .load(Ordering::Relaxed),
    })
}

pub(super) fn take_background_tasks(
    background_tasks: &Arc<Mutex<Vec<JoinHandle<()>>>>,
) -> Vec<JoinHandle<()>> {
    let mut tasks = background_tasks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    std::mem::take(&mut *tasks)
}
