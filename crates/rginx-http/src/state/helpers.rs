fn prepare_state(
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

fn prepare_listener_tls_acceptors(
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

fn build_peer_health_notifier(
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

fn unix_time_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn take_background_tasks(
    background_tasks: &Arc<Mutex<Vec<JoinHandle<()>>>>,
) -> Vec<JoinHandle<()>> {
    let mut tasks = background_tasks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    std::mem::take(&mut *tasks)
}
