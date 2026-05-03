use super::*;

impl CacheManager {
    pub(crate) fn from_config_with_notifier(
        config: &ConfigSnapshot,
        change_notifier: Option<CacheChangeNotifier>,
    ) -> Result<Self> {
        let zones = config
            .cache_zones
            .iter()
            .map(|(name, zone)| {
                std::fs::create_dir_all(&zone.path).map_err(|error| {
                    Error::Server(format!(
                        "failed to create cache zone `{name}` directory `{}`: {error}",
                        zone.path.display()
                    ))
                })?;
                let (
                    index,
                    shared_index_store,
                    shared_index_generation,
                    shared_index_store_epoch,
                    shared_index_change_seq,
                ) = bootstrap_shared_index(zone.as_ref()).map_err(|error| {
                    Error::Server(format!(
                        "failed to load cache zone `{name}` index from `{}`: {error}",
                        zone.path.display()
                    ))
                })?;
                Ok((
                    name.clone(),
                    Arc::new(CacheZoneRuntime {
                        config: zone.clone(),
                        index: RwLock::new(index),
                        hot_entries: RwLock::new(HashMap::new()),
                        io_locks: CacheIoLockPool::new(),
                        shared_index_sync_lock: AsyncMutex::new(()),
                        shared_index_store,
                        fill_locks: Arc::new(Mutex::new(HashMap::new())),
                        fill_lock_generation: AtomicU64::new(0),
                        last_inactive_cleanup_unix_ms: AtomicU64::new(0),
                        shared_index_generation: AtomicU64::new(shared_index_generation),
                        shared_index_store_epoch: AtomicU64::new(shared_index_store_epoch),
                        shared_index_change_seq: AtomicU64::new(shared_index_change_seq),
                        stats: CacheZoneStats::default(),
                        change_notifier: change_notifier.clone(),
                    }),
                ))
            })
            .collect::<Result<HashMap<_, _>>>()?;

        Ok(Self { zones: Arc::new(zones) })
    }
}
