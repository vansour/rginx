use super::*;
use crate::cache::shared::SHARED_ACCESS_TOUCH_PUBLISH_INTERVAL_MS;
use crate::cache::shared::run_blocking;

impl CacheZoneRuntime {
    pub(in crate::cache) fn snapshot(&self) -> CacheZoneRuntimeSnapshot {
        let (entry_count, current_size_bytes, active_invalidation_rules) = {
            let index = read_index(&self.index);
            (index.entries.len(), index.current_size_bytes, index.invalidations.len())
        };
        let shared_index_metrics = self
            .shared_index_store
            .as_ref()
            .and_then(|store| run_blocking(|| store.metrics()).ok())
            .unwrap_or_default();
        CacheZoneRuntimeSnapshot {
            zone_name: self.config.name.clone(),
            path: self.config.path.clone(),
            max_size_bytes: self.config.max_size_bytes,
            inactive_secs: self.config.inactive.as_secs(),
            default_ttl_secs: self.config.default_ttl.as_secs(),
            max_entry_bytes: self.config.max_entry_bytes,
            entry_count,
            current_size_bytes,
            hit_total: self.stats.hit_total.load(Ordering::Relaxed),
            miss_total: self.stats.miss_total.load(Ordering::Relaxed),
            bypass_total: self.stats.bypass_total.load(Ordering::Relaxed),
            expired_total: self.stats.expired_total.load(Ordering::Relaxed),
            stale_total: self.stats.stale_total.load(Ordering::Relaxed),
            updating_total: self.stats.updating_total.load(Ordering::Relaxed),
            revalidated_total: self.stats.revalidated_total.load(Ordering::Relaxed),
            write_success_total: self.stats.write_success_total.load(Ordering::Relaxed),
            write_error_total: self.stats.write_error_total.load(Ordering::Relaxed),
            eviction_total: self.stats.eviction_total.load(Ordering::Relaxed),
            purge_total: self.stats.purge_total.load(Ordering::Relaxed),
            invalidation_total: self.stats.invalidation_total.load(Ordering::Relaxed),
            inactive_cleanup_total: self.stats.inactive_cleanup_total.load(Ordering::Relaxed),
            active_invalidation_rules,
            shared_index_enabled: self.config.shared_index,
            shared_index_generation: self.shared_index_generation.load(Ordering::Relaxed),
            shared_index_shm_capacity_bytes: shared_index_metrics.shm_capacity_bytes,
            shared_index_shm_used_bytes: shared_index_metrics.shm_used_bytes,
            shared_index_entry_count: shared_index_metrics.entry_count,
            shared_index_current_size_bytes: shared_index_metrics.current_size_bytes,
            shared_index_operation_ring_capacity: shared_index_metrics.operation_ring_capacity,
            shared_index_operation_ring_used: shared_index_metrics.operation_ring_used,
            shared_index_lock_contention_total: shared_index_metrics.lock_contention_total,
            shared_index_full_reload_total: shared_index_metrics.full_reload_total,
            shared_index_rebuild_total: shared_index_metrics.rebuild_total,
            shared_index_stale_fill_lock_cleanup_total: shared_index_metrics
                .stale_fill_lock_cleanup_total,
            shared_index_capacity_rejection_total: shared_index_metrics.capacity_rejection_total,
        }
    }

    fn hot_entry(&self, key: &str) -> Option<Arc<CacheEntryHotState>> {
        self.hot_entries.read().unwrap_or_else(|poisoned| poisoned.into_inner()).get(key).cloned()
    }

    fn hot_entry_for_key(&self, key: &str) -> Arc<CacheEntryHotState> {
        if let Some(entry) = self.hot_entry(key) {
            return entry;
        }

        let mut hot_entries =
            self.hot_entries.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        hot_entries
            .entry(key.to_string())
            .or_insert_with(|| {
                Arc::new(CacheEntryHotState {
                    last_access_unix_ms: AtomicU64::new(0),
                    shared_touch_published_unix_ms: AtomicU64::new(0),
                    response_head: Mutex::new(None),
                })
            })
            .clone()
    }

    pub(in crate::cache) fn record_entry_access(&self, key: &str, now: u64) {
        self.hot_entry_for_key(key).last_access_unix_ms.fetch_max(now, Ordering::Relaxed);
    }

    pub(in crate::cache) fn should_publish_shared_entry_access(&self, key: &str, now: u64) -> bool {
        let hot_entry = self.hot_entry_for_key(key);
        loop {
            let last_published = hot_entry.shared_touch_published_unix_ms.load(Ordering::Relaxed);
            if last_published != 0
                && now.saturating_sub(last_published) < SHARED_ACCESS_TOUCH_PUBLISH_INTERVAL_MS
            {
                return false;
            }
            if hot_entry
                .shared_touch_published_unix_ms
                .compare_exchange(last_published, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return true;
            }
        }
    }

    pub(in crate::cache) fn effective_last_access_unix_ms(
        &self,
        key: &str,
        entry: &CacheIndexEntry,
    ) -> u64 {
        self.hot_entry(key)
            .map(|hot| hot.last_access_unix_ms.load(Ordering::Relaxed))
            .map_or(entry.last_access_unix_ms, |hot| hot.max(entry.last_access_unix_ms))
    }

    pub(in crate::cache) fn prepared_response_head(
        &self,
        key: &str,
        expected_hash: &str,
    ) -> Option<Arc<PreparedCacheResponseHead>> {
        let hot_entry = self.hot_entry(key)?;
        let response_head = hot_entry
            .response_head
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()?;
        (response_head.hash == expected_hash).then_some(response_head)
    }

    pub(in crate::cache) fn store_prepared_response_head(
        &self,
        key: &str,
        last_access_unix_ms: u64,
        response_head: Arc<PreparedCacheResponseHead>,
    ) {
        let still_current = read_index(&self.index)
            .entries
            .get(key)
            .is_some_and(|entry| entry.hash == response_head.hash);
        if !still_current {
            return;
        }

        let hot_entry = self.hot_entry_for_key(key);
        hot_entry.last_access_unix_ms.fetch_max(last_access_unix_ms, Ordering::Relaxed);
        *hot_entry.response_head.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(response_head);
    }

    pub(in crate::cache) fn remove_hot_entry(&self, key: &str) {
        self.hot_entries.write().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(key);
    }

    pub(in crate::cache) fn clear_hot_entries(&self) {
        self.hot_entries.write().unwrap_or_else(|poisoned| poisoned.into_inner()).clear();
    }

    pub(in crate::cache) fn record_hit(&self) {
        self.record_counter(&self.stats.hit_total, 1);
    }

    pub(in crate::cache) fn record_miss(&self) {
        self.record_counter(&self.stats.miss_total, 1);
    }

    pub(in crate::cache) fn record_bypass(&self) {
        self.record_counter(&self.stats.bypass_total, 1);
    }

    pub(in crate::cache) fn record_expired(&self) {
        self.record_counter(&self.stats.expired_total, 1);
    }

    pub(in crate::cache) fn record_stale(&self) {
        self.record_counter(&self.stats.stale_total, 1);
    }

    pub(in crate::cache) fn record_updating(&self) {
        self.record_counter(&self.stats.updating_total, 1);
    }

    pub(in crate::cache) fn record_revalidated(&self) {
        self.record_counter(&self.stats.revalidated_total, 1);
    }

    pub(in crate::cache) fn record_write_success(&self) {
        self.record_counter(&self.stats.write_success_total, 1);
    }

    pub(in crate::cache) fn record_write_error(&self) {
        self.record_counter(&self.stats.write_error_total, 1);
    }

    pub(in crate::cache) fn record_evictions(&self, count: usize) {
        if count > 0 {
            self.record_counter(&self.stats.eviction_total, count as u64);
        }
    }

    pub(in crate::cache) fn record_purge(&self, count: usize) {
        if count > 0 {
            self.record_counter(&self.stats.purge_total, count as u64);
        }
    }

    pub(in crate::cache) fn record_invalidation(&self, count: usize) {
        if count > 0 {
            self.record_counter(&self.stats.invalidation_total, count as u64);
        }
    }

    pub(in crate::cache) fn record_inactive_cleanup(&self, count: usize) {
        if count > 0 {
            self.record_counter(&self.stats.inactive_cleanup_total, count as u64);
        }
    }

    fn record_counter(&self, counter: &AtomicU64, value: u64) {
        counter.fetch_add(value, Ordering::Relaxed);
    }

    pub(in crate::cache) fn notify_changed(&self) {
        if let Some(notifier) = &self.change_notifier {
            notifier(&self.config.name);
        }
    }
}
