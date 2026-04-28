use super::*;

impl CacheManager {
    pub(super) fn lookup_decision(
        &self,
        zone: &Arc<CacheZoneRuntime>,
        key: &str,
        now: u64,
        request_forces_revalidation: bool,
    ) -> LookupDecision {
        let mut index = lock_index(&zone.index);
        match index.entries.get_mut(key) {
            Some(entry)
                if now <= entry.expires_at_unix_ms
                    && !entry.must_revalidate
                    && !request_forces_revalidation =>
            {
                entry.last_access_unix_ms = now;
                LookupDecision::FreshHit(entry.clone())
            }
            Some(entry) => {
                entry.last_access_unix_ms = now;
                match zone.fill_lock_decision(key) {
                    FillLockDecision::Acquired(fill_guard) => {
                        let expired = now > entry.expires_at_unix_ms;
                        let cache_status =
                            if expired { CacheStatus::Expired } else { CacheStatus::Revalidated };
                        let allow_stale_on_error = expired
                            && entry.stale_if_error_until_unix_ms.is_some_and(|until| now <= until);
                        LookupDecision::Miss {
                            cached_entry: Some(entry.clone()),
                            fill_guard,
                            cache_status,
                            allow_stale_on_error,
                        }
                    }
                    FillLockDecision::Wait(_waiter)
                        if now > entry.expires_at_unix_ms
                            && entry
                                .stale_while_revalidate_until_unix_ms
                                .is_some_and(|until| now <= until) =>
                    {
                        LookupDecision::StaleWhileRevalidate(entry.clone())
                    }
                    FillLockDecision::Wait(waiter) => LookupDecision::Wait(waiter),
                }
            }
            None => match zone.fill_lock_decision(key) {
                FillLockDecision::Acquired(fill_guard) => LookupDecision::Miss {
                    cached_entry: None,
                    fill_guard,
                    cache_status: CacheStatus::Miss,
                    allow_stale_on_error: false,
                },
                FillLockDecision::Wait(waiter) => LookupDecision::Wait(waiter),
            },
        }
    }

    pub(super) async fn load_lookup_metadata(
        &self,
        zone: &Arc<CacheZoneRuntime>,
        key: &str,
        entry: &CacheIndexEntry,
    ) -> Option<(CacheMetadata, Option<CacheConditionalHeaders>)> {
        let metadata = {
            let _io_guard = zone.io_lock.lock().await;
            let paths = cache_paths(&zone.config.path, &entry.hash);
            read_cache_metadata(&paths.metadata).await
        };
        let metadata = match metadata {
            Ok(metadata) => metadata,
            Err(error) => {
                tracing::warn!(
                    zone = %zone.config.name,
                    key_hash = %entry.hash,
                    %error,
                    "failed to read cache metadata; removing entry"
                );
                remove_index_entry(zone, key);
                remove_cache_files_if_unindexed(zone, key, &entry.hash).await;
                return None;
            }
        };
        if metadata.key != key {
            tracing::warn!(
                zone = %zone.config.name,
                key = %key,
                cached_key = %metadata.key,
                key_hash = %entry.hash,
                "cache metadata key mismatch; removing entry"
            );
            remove_index_entry(zone, key);
            remove_cache_files_if_unindexed(zone, key, &entry.hash).await;
            return None;
        }
        let headers = match metadata.headers_map() {
            Ok(headers) => headers,
            Err(error) => {
                tracing::warn!(
                    zone = %zone.config.name,
                    key_hash = %entry.hash,
                    %error,
                    "failed to decode cached response headers; removing entry"
                );
                remove_index_entry(zone, key);
                remove_cache_files_if_unindexed(zone, key, &entry.hash).await;
                return None;
            }
        };
        let conditional_headers = build_conditional_headers(&headers);
        Some((metadata, conditional_headers))
    }

    pub(super) async fn stale_response_from_entry(
        &self,
        zone: &Arc<CacheZoneRuntime>,
        key: &str,
        entry: &CacheIndexEntry,
        read_cached_body: bool,
    ) -> Option<HttpResponse> {
        let (metadata, response) = {
            let _io_guard = zone.io_lock.lock().await;
            let paths = cache_paths(&zone.config.path, &entry.hash);
            let metadata = read_cache_metadata(&paths.metadata).await.ok()?;
            let response =
                build_cached_response(&paths.body, &metadata, read_cached_body).await.ok()?;
            (metadata, response)
        };
        if metadata.key != key {
            tracing::warn!(
                zone = %zone.config.name,
                key = %key,
                cached_key = %metadata.key,
                key_hash = %entry.hash,
                "cache metadata key mismatch while serving stale entry; removing entry"
            );
            remove_index_entry(zone, key);
            remove_cache_files_if_unindexed(zone, key, &entry.hash).await;
            return None;
        }
        zone.record_stale();
        Some(with_cache_status(response, CacheStatus::Stale))
    }
}
