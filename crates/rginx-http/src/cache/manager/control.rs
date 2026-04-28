use super::*;

impl CacheManager {
    pub(crate) fn snapshot(&self) -> Vec<CacheZoneRuntimeSnapshot> {
        let mut snapshots = self.zones.values().map(|zone| zone.snapshot()).collect::<Vec<_>>();
        snapshots.sort_by(|left, right| left.zone_name.cmp(&right.zone_name));
        snapshots
    }

    pub(crate) async fn cleanup_inactive_entries(&self) {
        for zone in self.zones.values() {
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
        Ok(purge_zone_entries(zone, PurgeSelector::Prefix(prefix.to_string())).await)
    }
}
