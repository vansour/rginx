#[derive(Debug, Default)]
pub(crate) struct SnapshotComponentVersions {
    status: AtomicU64,
    counters: AtomicU64,
    traffic: AtomicU64,
    peer_health: AtomicU64,
    upstreams: AtomicU64,
    cache: AtomicU64,
}
