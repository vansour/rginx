fn prepare_state(
    config: ConfigSnapshot,
    peer_health_notifier: Option<HealthChangeNotifier>,
) -> Result<PreparedState> {
    let config = Arc::new(config);
    let clients =
        ProxyClients::from_config_with_health_notifier(config.as_ref(), peer_health_notifier)?;
    let listener_tls_acceptors = config
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
        .collect::<Result<HashMap<_, _>>>()?;

    Ok(PreparedState { config, clients, listener_tls_acceptors })
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

pub fn validate_config_transition(current: &ConfigSnapshot, next: &ConfigSnapshot) -> Result<()> {
    let mut changes = Vec::new();

    if current.listeners.len() != next.listeners.len() {
        changes.push(format!(
            "listeners count {} -> {}",
            current.listeners.len(),
            next.listeners.len()
        ));
    }

    for (current_listener, next_listener) in current.listeners.iter().zip(next.listeners.iter()) {
        if current_listener.id != next_listener.id {
            changes.push(format!("listener id {} -> {}", current_listener.id, next_listener.id));
        }

        if current_listener.server.listen_addr != next_listener.server.listen_addr {
            changes.push(format!(
                "{}.listen {} -> {}",
                current_listener.id,
                current_listener.server.listen_addr,
                next_listener.server.listen_addr
            ));
        }
    }

    if current.runtime.worker_threads != next.runtime.worker_threads {
        changes.push(format!(
            "runtime.worker_threads {:?} -> {:?}",
            current.runtime.worker_threads, next.runtime.worker_threads
        ));
    }

    if current.runtime.accept_workers != next.runtime.accept_workers {
        changes.push(format!(
            "runtime.accept_workers {} -> {}",
            current.runtime.accept_workers, next.runtime.accept_workers
        ));
    }

    if changes.is_empty() {
        return Ok(());
    }

    Err(Error::Config(format!(
        "reload requires restart because these startup-boundary fields changed (restart-boundary: {}): {}",
        tls_restart_required_fields().join(", "),
        changes.join("; ")
    )))
}

fn take_background_tasks(
    background_tasks: &Arc<Mutex<Vec<JoinHandle<()>>>>,
) -> Vec<JoinHandle<()>> {
    let mut tasks = background_tasks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    std::mem::take(&mut *tasks)
}
