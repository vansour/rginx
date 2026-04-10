use super::*;

impl SharedState {
    pub async fn snapshot(&self) -> ActiveState {
        self.inner.read().await.clone()
    }

    pub async fn current_config(&self) -> Arc<ConfigSnapshot> {
        self.inner.read().await.config.clone()
    }

    pub async fn current_revision(&self) -> u64 {
        self.inner.read().await.revision
    }

    pub fn current_snapshot_version(&self) -> u64 {
        self.snapshot_version.load(Ordering::Relaxed)
    }

    pub async fn current_listener(&self, listener_id: &str) -> Option<Listener> {
        if let Some(listener) = self.inner.read().await.config.listener(listener_id).cloned() {
            return Some(listener);
        }

        self.retired_listeners
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(listener_id)
            .cloned()
    }

    pub fn config_path(&self) -> Option<&PathBuf> {
        self.config_path.as_deref()
    }

    pub fn subscribe_updates(&self) -> watch::Receiver<u64> {
        self.revisions.subscribe()
    }

    pub async fn wait_for_snapshot_change(
        &self,
        since_version: u64,
        timeout: Option<std::time::Duration>,
    ) -> u64 {
        loop {
            let notified = self.snapshot_notify.notified();
            let current = self.current_snapshot_version();
            if current > since_version {
                return current;
            }

            if let Some(timeout) = timeout {
                if tokio::time::timeout(timeout, notified).await.is_err() {
                    return self.current_snapshot_version();
                }
            } else {
                notified.await;
            }
        }
    }

    pub fn snapshot_delta_since(
        &self,
        since_version: u64,
        include: Option<&[SnapshotModule]>,
        window_secs: Option<u64>,
    ) -> SnapshotDeltaSnapshot {
        let included_modules = SnapshotModule::normalize(include);
        let current_snapshot_version = self.current_snapshot_version();
        let status_version = self.snapshot_components.status.load(Ordering::Relaxed);
        let counters_version = self.snapshot_components.counters.load(Ordering::Relaxed);
        let traffic_version = self.snapshot_components.traffic.load(Ordering::Relaxed);
        let peer_health_version = self.snapshot_components.peer_health.load(Ordering::Relaxed);
        let upstreams_version = self.snapshot_components.upstreams.load(Ordering::Relaxed);
        let (changed_listener_ids, changed_vhost_ids, changed_route_ids) =
            self.changed_traffic_targets_since(since_version);
        let changed_peer_health_upstream_names = self.changed_named_component_targets_since(
            &self.peer_health_component_versions,
            since_version,
        );
        let changed_upstream_names = self.changed_named_component_targets_since(
            &self.upstream_component_versions,
            since_version,
        );

        SnapshotDeltaSnapshot {
            schema_version: 2,
            since_version,
            current_snapshot_version,
            included_modules: included_modules.clone(),
            recent_window_secs: window_secs,
            status_version: included_modules
                .contains(&SnapshotModule::Status)
                .then_some(status_version),
            counters_version: included_modules
                .contains(&SnapshotModule::Counters)
                .then_some(counters_version),
            traffic_version: included_modules
                .contains(&SnapshotModule::Traffic)
                .then_some(traffic_version),
            peer_health_version: included_modules
                .contains(&SnapshotModule::PeerHealth)
                .then_some(peer_health_version),
            upstreams_version: included_modules
                .contains(&SnapshotModule::Upstreams)
                .then_some(upstreams_version),
            status_changed: included_modules
                .contains(&SnapshotModule::Status)
                .then_some(status_version > since_version),
            counters_changed: included_modules
                .contains(&SnapshotModule::Counters)
                .then_some(counters_version > since_version),
            traffic_changed: included_modules
                .contains(&SnapshotModule::Traffic)
                .then_some(traffic_version > since_version),
            traffic_recent_changed: (included_modules.contains(&SnapshotModule::Traffic)
                && window_secs.is_some())
            .then_some(traffic_version > since_version),
            peer_health_changed: included_modules
                .contains(&SnapshotModule::PeerHealth)
                .then_some(peer_health_version > since_version),
            upstreams_changed: included_modules
                .contains(&SnapshotModule::Upstreams)
                .then_some(upstreams_version > since_version),
            upstreams_recent_changed: (included_modules.contains(&SnapshotModule::Upstreams)
                && window_secs.is_some())
            .then_some(upstreams_version > since_version),
            changed_listener_ids: included_modules
                .contains(&SnapshotModule::Traffic)
                .then_some(changed_listener_ids.clone()),
            changed_vhost_ids: included_modules
                .contains(&SnapshotModule::Traffic)
                .then_some(changed_vhost_ids.clone()),
            changed_route_ids: included_modules
                .contains(&SnapshotModule::Traffic)
                .then_some(changed_route_ids.clone()),
            changed_recent_listener_ids: (included_modules.contains(&SnapshotModule::Traffic)
                && window_secs.is_some())
            .then_some(changed_listener_ids.clone()),
            changed_recent_vhost_ids: (included_modules.contains(&SnapshotModule::Traffic)
                && window_secs.is_some())
            .then_some(changed_vhost_ids.clone()),
            changed_recent_route_ids: (included_modules.contains(&SnapshotModule::Traffic)
                && window_secs.is_some())
            .then_some(changed_route_ids.clone()),
            changed_peer_health_upstream_names: included_modules
                .contains(&SnapshotModule::PeerHealth)
                .then_some(changed_peer_health_upstream_names.clone()),
            changed_upstream_names: included_modules
                .contains(&SnapshotModule::Upstreams)
                .then_some(changed_upstream_names.clone()),
            changed_recent_upstream_names: (included_modules.contains(&SnapshotModule::Upstreams)
                && window_secs.is_some())
            .then_some(changed_upstream_names.clone()),
        }
    }

    pub(crate) fn changed_named_component_targets_since(
        &self,
        versions: &Arc<StdRwLock<HashMap<String, u64>>>,
        since_version: u64,
    ) -> Vec<String> {
        let versions = versions.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut changed = versions
            .iter()
            .filter_map(|(name, version)| (*version > since_version).then_some(name.clone()))
            .collect::<Vec<_>>();
        changed.sort();
        changed
    }

    pub(crate) fn mark_named_component_target_changed(
        &self,
        versions: &Arc<StdRwLock<HashMap<String, u64>>>,
        name: &str,
        version: u64,
    ) {
        versions
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(name.to_string(), version);
    }

    pub(crate) fn mark_snapshot_changed_components(
        &self,
        status: bool,
        counters: bool,
        traffic: bool,
        peer_health: bool,
        upstreams: bool,
    ) -> u64 {
        let version = self.snapshot_version.fetch_add(1, Ordering::Relaxed) + 1;
        if status {
            self.snapshot_components.status.store(version, Ordering::Relaxed);
        }
        if counters {
            self.snapshot_components.counters.store(version, Ordering::Relaxed);
        }
        if traffic {
            self.snapshot_components.traffic.store(version, Ordering::Relaxed);
        }
        if peer_health {
            self.snapshot_components.peer_health.store(version, Ordering::Relaxed);
        }
        if upstreams {
            self.snapshot_components.upstreams.store(version, Ordering::Relaxed);
        }
        version
    }

    pub(crate) fn notify_snapshot_waiters(&self) {
        self.snapshot_notify.notify_waiters();
    }
}
