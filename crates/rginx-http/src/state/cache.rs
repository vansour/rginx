use super::*;

impl SharedState {
    pub async fn cache_stats_snapshot(&self) -> CacheStatsSnapshot {
        let cache = {
            let state = self.inner.read().await;
            state.cache.clone()
        };
        CacheStatsSnapshot { zones: cache.snapshot_with_shared_sync().await }
    }

    pub async fn cleanup_cache_inactive_entries(&self) {
        let cache = {
            let state = self.inner.read().await;
            state.cache.clone()
        };
        cache.cleanup_inactive_entries().await;
    }

    pub async fn purge_cache_zone(
        &self,
        zone_name: &str,
    ) -> std::result::Result<CachePurgeResult, String> {
        let cache = {
            let state = self.inner.read().await;
            state.cache.clone()
        };
        cache.purge_zone(zone_name).await
    }

    pub async fn purge_cache_key(
        &self,
        zone_name: &str,
        key: &str,
    ) -> std::result::Result<CachePurgeResult, String> {
        let cache = {
            let state = self.inner.read().await;
            state.cache.clone()
        };
        cache.purge_key(zone_name, key).await
    }

    pub async fn purge_cache_prefix(
        &self,
        zone_name: &str,
        prefix: &str,
    ) -> std::result::Result<CachePurgeResult, String> {
        let cache = {
            let state = self.inner.read().await;
            state.cache.clone()
        };
        cache.purge_prefix(zone_name, prefix).await
    }

    pub(crate) fn sync_cache_versions(&self, config: &ConfigSnapshot) {
        let existing =
            self.cache_component_versions.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let next = build_cache_zone_versions(config, Some(&*existing));
        drop(existing);
        *self.cache_component_versions.write().unwrap_or_else(|poisoned| poisoned.into_inner()) =
            next;
    }

    pub(crate) fn mark_all_cache_zones_changed(
        &self,
        previous: &ConfigSnapshot,
        next: &ConfigSnapshot,
        version: u64,
    ) {
        let mut cache_versions =
            self.cache_component_versions.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        for zone_name in previous.cache_zones.keys() {
            cache_versions.insert(zone_name.clone(), version);
        }
        for zone_name in next.cache_zones.keys() {
            cache_versions.insert(zone_name.clone(), version);
        }
    }
}

pub(super) fn build_cache_zone_versions(
    config: &ConfigSnapshot,
    existing: Option<&HashMap<String, u64>>,
) -> HashMap<String, u64> {
    config
        .cache_zones
        .keys()
        .map(|name| {
            let version = existing.and_then(|current| current.get(name)).copied().unwrap_or(0);
            (name.clone(), version)
        })
        .collect()
}
