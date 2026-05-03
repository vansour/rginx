use super::invalidation::entry_is_logically_invalid;
use super::runtime::{CacheEntryLifecyclePhase, lifecycle_phase};
use super::*;

mod helpers;

use helpers::{fill_share_fingerprint, matching_variant_key, stale_allowed_for_entry};

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
        let matching = {
            let index = read_index(&zone.index);
            matching_variant_key(&index, base_key, request).and_then(|key| {
                index.entries.get(&key).cloned().map(|entry| {
                    let invalidated = entry_is_logically_invalid(&index, &key, &entry);
                    (key, entry, invalidated)
                })
            })
        };
        let share_fingerprint = fill_share_fingerprint(request);

        match matching {
            Some((key, entry, invalidated)) => {
                if invalidated {
                    return LookupDecision::DropEntry { key, entry };
                }
                let phase = lifecycle_phase(&entry, now);
                if entry.is_hit_for_pass() {
                    return if phase == CacheEntryLifecyclePhase::Dead {
                        LookupDecision::DropEntry { key, entry }
                    } else {
                        LookupDecision::Bypass { status: CacheStatus::Bypass }
                    };
                }
                if phase == CacheEntryLifecyclePhase::Dead {
                    return LookupDecision::DropEntry { key, entry };
                }

                zone.record_entry_access(&key, now);

                if phase == CacheEntryLifecyclePhase::Fresh
                    && !entry.requires_revalidation
                    && !request_forces_revalidation
                {
                    return LookupDecision::FreshHit { key, entry };
                }

                let expired = now > entry.expires_at_unix_ms;
                match zone.fill_lock_decision(&key, now, policy.lock_age, None) {
                    FillLockDecision::Acquired(fill_guard) => {
                        if expired
                            && stale_allowed_for_entry(
                                policy,
                                &entry,
                                now,
                                request_forces_revalidation,
                            )
                            && policy.background_update
                        {
                            LookupDecision::BackgroundUpdate {
                                key,
                                cached_entry: entry,
                                fill_guard,
                            }
                        } else {
                            LookupDecision::Miss {
                                key,
                                base_key: entry.base_key.clone(),
                                cached_entry: Some(entry),
                                fill_guard: Some(fill_guard),
                                cache_status: if expired {
                                    CacheStatus::Expired
                                } else {
                                    CacheStatus::Revalidated
                                },
                            }
                        }
                    }
                    FillLockDecision::WaitLocal { waiter: _waiter }
                        if expired
                            && stale_allowed_for_entry(
                                policy,
                                &entry,
                                now,
                                request_forces_revalidation,
                            ) =>
                    {
                        LookupDecision::Stale { key, entry, status: CacheStatus::Updating }
                    }
                    FillLockDecision::WaitExternal { key: _external_key }
                        if expired
                            && stale_allowed_for_entry(
                                policy,
                                &entry,
                                now,
                                request_forces_revalidation,
                            ) =>
                    {
                        LookupDecision::Stale { key, entry, status: CacheStatus::Updating }
                    }
                    FillLockDecision::ReadLocal { state: _state }
                        if expired
                            && stale_allowed_for_entry(
                                policy,
                                &entry,
                                now,
                                request_forces_revalidation,
                            ) =>
                    {
                        LookupDecision::Stale { key, entry, status: CacheStatus::Updating }
                    }
                    FillLockDecision::ReadExternal { state: _state }
                        if expired
                            && stale_allowed_for_entry(
                                policy,
                                &entry,
                                now,
                                request_forces_revalidation,
                            ) =>
                    {
                        LookupDecision::Stale { key, entry, status: CacheStatus::Updating }
                    }
                    FillLockDecision::ReadLocal { state } => {
                        LookupDecision::ReadWhileFillLocal { state }
                    }
                    FillLockDecision::ReadExternal { state } => {
                        LookupDecision::ReadWhileFillExternal { state }
                    }
                    FillLockDecision::WaitLocal { waiter } => {
                        LookupDecision::Wait { strategy: LookupWait::Local { waiter } }
                    }
                    FillLockDecision::WaitExternal { key } => {
                        LookupDecision::Wait { strategy: LookupWait::External { key } }
                    }
                }
            }
            None => match zone.fill_lock_decision(
                base_key,
                now,
                policy.lock_age,
                Some(&share_fingerprint),
            ) {
                FillLockDecision::Acquired(fill_guard) => LookupDecision::Miss {
                    key: base_key.to_string(),
                    base_key: base_key.to_string(),
                    cached_entry: None,
                    fill_guard: Some(fill_guard),
                    cache_status: CacheStatus::Miss,
                },
                FillLockDecision::ReadLocal { state } => {
                    LookupDecision::ReadWhileFillLocal { state }
                }
                FillLockDecision::ReadExternal { state } => {
                    LookupDecision::ReadWhileFillExternal { state }
                }
                FillLockDecision::WaitLocal { waiter } => {
                    LookupDecision::Wait { strategy: LookupWait::Local { waiter } }
                }
                FillLockDecision::WaitExternal { key } => {
                    LookupDecision::Wait { strategy: LookupWait::External { key } }
                }
            },
        }
    }

    pub(super) async fn load_lookup_response_head(
        &self,
        zone: &Arc<CacheZoneRuntime>,
        key: &str,
        entry: &CacheIndexEntry,
    ) -> Option<Arc<PreparedCacheResponseHead>> {
        match load_cached_response_head(zone, key, entry).await {
            Ok(response_head) => Some(response_head),
            Err(error) => {
                tracing::warn!(
                    zone = %zone.config.name,
                    key = %key,
                    key_hash = %entry.hash,
                    %error,
                    "failed to load cached response head; removing entry"
                );
                remove_cache_entry_if_matches(zone, key, entry).await;
                None
            }
        }
    }

    pub(super) async fn stale_response_from_entry(
        &self,
        zone: &Arc<CacheZoneRuntime>,
        key: &str,
        entry: &CacheIndexEntry,
        request: &CacheRequest,
        policy: &RouteCachePolicy,
        read_cached_body: bool,
        status: CacheStatus,
    ) -> Option<HttpResponse> {
        let response_head = match load_cached_response_head(zone, key, entry).await {
            Ok(response_head) => response_head,
            Err(error) => {
                tracing::warn!(
                    zone = %zone.config.name,
                    key = %key,
                    key_hash = %entry.hash,
                    %error,
                    "failed to load stale cached response head; removing entry"
                );
                remove_cache_entry_if_matches(zone, key, entry).await;
                return None;
            }
        };

        let response = {
            let _io_guard = zone.io_read(&entry.hash).await;
            let paths = cache_paths_for_zone(zone.config.as_ref(), &entry.hash);
            build_cached_response_for_request(
                &paths.body,
                response_head.as_ref(),
                request,
                policy,
                read_cached_body,
            )
            .await
        };
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(
                    zone = %zone.config.name,
                    key = %key,
                    key_hash = %entry.hash,
                    %error,
                    "failed to build stale cached response; removing entry"
                );
                remove_cache_entry_if_matches(zone, key, entry).await;
                return None;
            }
        };
        record_zone_shared_entry_access(zone, key, unix_time_ms(SystemTime::now())).await;
        if status == CacheStatus::Updating {
            zone.record_updating();
        } else {
            zone.record_stale();
        }
        Some(with_cache_status(response, status))
    }
}
