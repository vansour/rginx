use super::vary::matches_vary_headers;
use super::*;

impl CacheManager {
    pub(super) fn lookup_decision(
        &self,
        zone: &Arc<CacheZoneRuntime>,
        request: &CacheRequest,
        base_key: &str,
        now: u64,
        request_forces_revalidation: bool,
        policy: &RouteCachePolicy,
    ) -> LookupDecision {
        let mut index = lock_index(&zone.index);
        let matching_key = matching_variant_key(&index, base_key, request);

        match matching_key {
            Some(key) => {
                let entry = index
                    .entries
                    .get_mut(&key)
                    .expect("matching cache key should still be present in the index");
                entry.last_access_unix_ms = now;

                if now <= entry.expires_at_unix_ms
                    && !entry.must_revalidate
                    && !request_forces_revalidation
                {
                    return LookupDecision::FreshHit { key, entry: entry.clone() };
                }

                let expired = now > entry.expires_at_unix_ms;
                match zone.fill_lock_decision(&key, now, policy.lock_age) {
                    FillLockDecision::Acquired(fill_guard) => {
                        if expired
                            && stale_allowed_for_entry(policy, entry, now)
                            && policy.background_update
                        {
                            LookupDecision::BackgroundUpdate {
                                key,
                                cached_entry: entry.clone(),
                                fill_guard,
                            }
                        } else {
                            LookupDecision::Miss {
                                key,
                                base_key: entry.base_key.clone(),
                                cached_entry: Some(entry.clone()),
                                fill_guard: Some(fill_guard),
                                cache_status: if expired {
                                    CacheStatus::Expired
                                } else {
                                    CacheStatus::Revalidated
                                },
                            }
                        }
                    }
                    FillLockDecision::Wait { waiter: _waiter }
                        if expired && stale_allowed_for_entry(policy, entry, now) =>
                    {
                        LookupDecision::Stale {
                            key,
                            entry: entry.clone(),
                            status: CacheStatus::Updating,
                        }
                    }
                    FillLockDecision::Wait { waiter } => LookupDecision::Wait { waiter },
                }
            }
            None => match zone.fill_lock_decision(base_key, now, policy.lock_age) {
                FillLockDecision::Acquired(fill_guard) => LookupDecision::Miss {
                    key: base_key.to_string(),
                    base_key: base_key.to_string(),
                    cached_entry: None,
                    fill_guard: Some(fill_guard),
                    cache_status: CacheStatus::Miss,
                },
                FillLockDecision::Wait { waiter } => LookupDecision::Wait { waiter },
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
            let paths = cache_paths_for_zone(zone.config.as_ref(), &entry.hash);
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
        status: CacheStatus,
    ) -> Option<HttpResponse> {
        let (metadata, response) = {
            let _io_guard = zone.io_lock.lock().await;
            let paths = cache_paths_for_zone(zone.config.as_ref(), &entry.hash);
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
        if status == CacheStatus::Updating {
            zone.record_updating();
        } else {
            zone.record_stale();
        }
        Some(with_cache_status(response, status))
    }
}

fn matching_variant_key(
    index: &CacheIndex,
    base_key: &str,
    request: &CacheRequest,
) -> Option<String> {
    if !index.variants.contains_key(base_key)
        && index
            .entries
            .get(base_key)
            .is_some_and(|entry| matches_vary_headers(request, &entry.vary))
    {
        return Some(base_key.to_string());
    }
    index
        .variants
        .get(base_key)
        .into_iter()
        .flatten()
        .find(|candidate_key| {
            index
                .entries
                .get(*candidate_key)
                .is_some_and(|entry| matches_vary_headers(request, &entry.vary))
        })
        .cloned()
}

fn stale_allowed_for_entry(policy: &RouteCachePolicy, entry: &CacheIndexEntry, now: u64) -> bool {
    policy.use_stale.contains(&rginx_core::CacheUseStaleCondition::Updating)
        || entry.stale_while_revalidate_until_unix_ms.is_some_and(|until| now <= until)
}
