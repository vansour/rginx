use super::*;
use crate::cache::invalidation::normalize_cache_tag;
use crate::cache::store::{clear_zone_invalidations, invalidate_zone_entries};

impl CacheManager {
    pub(crate) fn snapshot(&self) -> Vec<CacheZoneRuntimeSnapshot> {
        let mut snapshots = self.zones.values().map(|zone| zone.snapshot()).collect::<Vec<_>>();
        snapshots.sort_by(|left, right| left.zone_name.cmp(&right.zone_name));
        snapshots
    }

    pub(crate) async fn snapshot_with_shared_sync(&self) -> Vec<CacheZoneRuntimeSnapshot> {
        for zone in self.zones.values() {
            sync_zone_shared_index_if_needed(zone).await;
        }
        self.snapshot()
    }

    pub(crate) async fn cleanup_inactive_entries(&self) {
        for zone in self.zones.values() {
            sync_zone_shared_index_if_needed(zone).await;
            cleanup_inactive_entries_in_zone(zone).await;
        }
    }

    pub(crate) async fn purge_zone(
        &self,
        zone_name: &str,
    ) -> std::result::Result<CachePurgeResult, String> {
        let zone = self
            .zones
            .get(zone_name)
            .cloned()
            .ok_or_else(|| format!("unknown cache zone `{zone_name}`"))?;
        sync_zone_shared_index_if_needed(&zone).await;
        Ok(purge_zone_entries(zone, PurgeSelector::All).await)
    }

    pub(crate) async fn purge_key(
        &self,
        zone_name: &str,
        key: &str,
    ) -> std::result::Result<CachePurgeResult, String> {
        let zone = self
            .zones
            .get(zone_name)
            .cloned()
            .ok_or_else(|| format!("unknown cache zone `{zone_name}`"))?;
        sync_zone_shared_index_if_needed(&zone).await;
        Ok(purge_zone_entries(zone, PurgeSelector::Exact(key.to_string())).await)
    }

    pub(crate) async fn purge_prefix(
        &self,
        zone_name: &str,
        prefix: &str,
    ) -> std::result::Result<CachePurgeResult, String> {
        let zone = self
            .zones
            .get(zone_name)
            .cloned()
            .ok_or_else(|| format!("unknown cache zone `{zone_name}`"))?;
        sync_zone_shared_index_if_needed(&zone).await;
        Ok(purge_zone_entries(zone, PurgeSelector::Prefix(prefix.to_string())).await)
    }

    pub(crate) async fn invalidate_zone(
        &self,
        zone_name: &str,
    ) -> std::result::Result<CacheInvalidationResult, String> {
        let zone = self
            .zones
            .get(zone_name)
            .cloned()
            .ok_or_else(|| format!("unknown cache zone `{zone_name}`"))?;
        sync_zone_shared_index_if_needed(&zone).await;
        Ok(invalidate_zone_entries(zone, CacheInvalidationSelector::All).await)
    }

    pub(crate) async fn invalidate_key(
        &self,
        zone_name: &str,
        key: &str,
    ) -> std::result::Result<CacheInvalidationResult, String> {
        let zone = self
            .zones
            .get(zone_name)
            .cloned()
            .ok_or_else(|| format!("unknown cache zone `{zone_name}`"))?;
        sync_zone_shared_index_if_needed(&zone).await;
        Ok(invalidate_zone_entries(zone, CacheInvalidationSelector::Exact(key.to_string())).await)
    }

    pub(crate) async fn invalidate_prefix(
        &self,
        zone_name: &str,
        prefix: &str,
    ) -> std::result::Result<CacheInvalidationResult, String> {
        let zone = self
            .zones
            .get(zone_name)
            .cloned()
            .ok_or_else(|| format!("unknown cache zone `{zone_name}`"))?;
        sync_zone_shared_index_if_needed(&zone).await;
        Ok(invalidate_zone_entries(zone, CacheInvalidationSelector::Prefix(prefix.to_string()))
            .await)
    }

    pub(crate) async fn invalidate_tag(
        &self,
        zone_name: &str,
        tag: &str,
    ) -> std::result::Result<CacheInvalidationResult, String> {
        let zone = self
            .zones
            .get(zone_name)
            .cloned()
            .ok_or_else(|| format!("unknown cache zone `{zone_name}`"))?;
        sync_zone_shared_index_if_needed(&zone).await;
        let Some(tag) = normalize_cache_tag(tag) else {
            return Err("cache invalidation tag must not be empty".to_string());
        };
        Ok(invalidate_zone_entries(zone, CacheInvalidationSelector::Tag(tag)).await)
    }

    pub(crate) async fn clear_invalidations(
        &self,
        zone_name: &str,
    ) -> std::result::Result<CacheInvalidationResult, String> {
        let zone = self
            .zones
            .get(zone_name)
            .cloned()
            .ok_or_else(|| format!("unknown cache zone `{zone_name}`"))?;
        sync_zone_shared_index_if_needed(&zone).await;
        Ok(clear_zone_invalidations(zone).await)
    }

    pub(crate) fn record_bypass_for_zone(&self, zone_name: &str) {
        if let Some(zone) = self.zones.get(zone_name) {
            zone.record_bypass();
        }
    }
}
