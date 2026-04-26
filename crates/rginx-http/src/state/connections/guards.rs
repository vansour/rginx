use super::super::*;

pub struct ActiveConnectionGuard {
    pub(super) active_connections: Arc<AtomicUsize>,
    pub(super) listener_active_connections: Arc<AtomicUsize>,
    pub(super) listener_id: String,
    pub(super) snapshot_version: Arc<AtomicU64>,
    pub(super) snapshot_notify: Arc<Notify>,
    pub(super) snapshot_components: Arc<SnapshotComponentVersions>,
    pub(super) traffic_component_versions: Arc<StdRwLock<TrafficComponentVersions>>,
}

pub(crate) struct ActiveHttp3ConnectionGuard {
    pub(super) counters: Arc<ListenerTrafficCounters>,
    pub(super) listener_id: String,
    pub(super) snapshot_version: Arc<AtomicU64>,
    pub(super) snapshot_notify: Arc<Notify>,
    pub(super) snapshot_components: Arc<SnapshotComponentVersions>,
    pub(super) traffic_component_versions: Arc<StdRwLock<TrafficComponentVersions>>,
}

pub(crate) struct ActiveHttp3RequestStreamGuard {
    pub(super) counters: Arc<ListenerTrafficCounters>,
    pub(super) listener_id: String,
    pub(super) snapshot_version: Arc<AtomicU64>,
    pub(super) snapshot_notify: Arc<Notify>,
    pub(super) snapshot_components: Arc<SnapshotComponentVersions>,
    pub(super) traffic_component_versions: Arc<StdRwLock<TrafficComponentVersions>>,
}

impl Drop for ActiveConnectionGuard {
    fn drop(&mut self) {
        self.active_connections.fetch_sub(1, Ordering::AcqRel);
        self.listener_active_connections.fetch_sub(1, Ordering::AcqRel);
        let version = self.snapshot_version.fetch_add(1, Ordering::Relaxed) + 1;
        self.snapshot_components.status.store(version, Ordering::Relaxed);
        self.snapshot_components.traffic.store(version, Ordering::Relaxed);
        self.traffic_component_versions
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .listeners
            .insert(self.listener_id.clone(), version);
        self.snapshot_notify.notify_waiters();
    }
}

impl Drop for ActiveHttp3ConnectionGuard {
    fn drop(&mut self) {
        self.counters.active_http3_connections.fetch_sub(1, Ordering::AcqRel);
        let version = self.snapshot_version.fetch_add(1, Ordering::Relaxed) + 1;
        self.snapshot_components.status.store(version, Ordering::Relaxed);
        self.snapshot_components.traffic.store(version, Ordering::Relaxed);
        self.traffic_component_versions
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .listeners
            .insert(self.listener_id.clone(), version);
        self.snapshot_notify.notify_waiters();
    }
}

impl Drop for ActiveHttp3RequestStreamGuard {
    fn drop(&mut self) {
        self.counters.active_http3_request_streams.fetch_sub(1, Ordering::AcqRel);
        let version = self.snapshot_version.fetch_add(1, Ordering::Relaxed) + 1;
        self.snapshot_components.status.store(version, Ordering::Relaxed);
        self.snapshot_components.traffic.store(version, Ordering::Relaxed);
        self.traffic_component_versions
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .listeners
            .insert(self.listener_id.clone(), version);
        self.snapshot_notify.notify_waiters();
    }
}
