use super::*;

mod bootstrap;
mod control;
mod response;

impl CacheManager {
    pub(crate) async fn lookup(
        &self,
        request: CacheRequest,
        downstream_scheme: &str,
        policy: &RouteCachePolicy,
    ) -> CacheLookup {
        let Some(zone) = self.zones.get(&policy.zone).cloned() else {
            tracing::warn!(
                zone = %policy.zone,
                "cache policy references unknown zone; bypassing cache"
            );
            return CacheLookup::Bypass(CacheStatus::Bypass);
        };

        if cache_request_bypass(&request, policy) {
            zone.record_bypass();
            return CacheLookup::Bypass(CacheStatus::Bypass);
        }

        sync_zone_shared_index_if_needed(&zone).await;

        let base_key = render_cache_key(
            &request.method,
            &request.uri,
            &request.headers,
            downstream_scheme,
            policy,
        );
        let request_forces_revalidation = request_requires_revalidation(&request.headers);
        let read_cached_body = request.method != Method::HEAD;

        loop {
            let now = unix_time_ms(SystemTime::now());
            match self.lookup_decision(
                &zone,
                &request,
                &base_key,
                now,
                request_forces_revalidation,
                policy,
            ) {
                LookupDecision::Bypass { status } => {
                    zone.record_bypass();
                    return CacheLookup::Bypass(status);
                }
                LookupDecision::DropEntry { key, entry } => {
                    remove_cache_entry_if_matches(&zone, &key, &entry).await;
                    continue;
                }
                LookupDecision::FreshHit { key, entry } => {
                    let cached_response = read_cached_response_for_request(
                        &zone,
                        &key,
                        &entry,
                        &request,
                        policy,
                        read_cached_body,
                    )
                    .await;
                    match cached_response {
                        Ok(mut response) => {
                            record_zone_shared_entry_access(
                                &zone,
                                &key,
                                unix_time_ms(SystemTime::now()),
                            )
                            .await;
                            zone.record_hit();
                            response
                                .headers_mut()
                                .insert(CACHE_STATUS_HEADER, CacheStatus::Hit.as_header_value());
                            return CacheLookup::Hit(response);
                        }
                        Err(error) => {
                            tracing::warn!(
                                zone = %zone.config.name,
                                key_hash = %entry.hash,
                                %error,
                                "failed to read cached response; treating as miss"
                            );
                            remove_cache_entry_if_matches(&zone, &key, &entry).await;
                        }
                    }
                }
                LookupDecision::Stale { key, entry, status } => {
                    match self
                        .stale_response_from_entry(
                            &zone,
                            &key,
                            &entry,
                            &request,
                            policy,
                            read_cached_body,
                            status,
                        )
                        .await
                    {
                        Some(response) => return CacheLookup::Hit(response),
                        None => continue,
                    }
                }
                LookupDecision::Wait { strategy } => {
                    let released = match strategy {
                        LookupWait::Local { waiter } => {
                            tokio::time::timeout(policy.lock_timeout, waiter).await.is_ok()
                        }
                        LookupWait::External { key } => {
                            zone.wait_for_external_fill_lock(
                                &key,
                                policy.lock_timeout,
                                policy.lock_age,
                            )
                            .await
                        }
                    };
                    if released {
                        sync_zone_shared_index_if_needed(&zone).await;
                        continue;
                    }
                    zone.record_bypass();
                    return CacheLookup::Miss(Box::new(CacheStoreContext {
                        zone,
                        policy: policy.clone(),
                        request,
                        base_key: base_key.clone(),
                        key: base_key.clone(),
                        cache_status: CacheStatus::Bypass,
                        store_response: false,
                        _fill_guard: None,
                        cached_entry: None,
                        cached_response_head: None,
                        revalidating: false,
                        request_forces_revalidation,
                        read_cached_body,
                    }));
                }
                LookupDecision::BackgroundUpdate { key, cached_entry, fill_guard } => {
                    let cached_response_head =
                        match self.load_lookup_response_head(&zone, &key, &cached_entry).await {
                            Some(response_head) => Some(response_head),
                            None => {
                                drop(fill_guard);
                                continue;
                            }
                        };
                    zone.record_expired();
                    let context = Box::new(CacheStoreContext {
                        zone: zone.clone(),
                        policy: policy.clone(),
                        request: request.with_method(Method::GET),
                        base_key: cached_entry.base_key.clone(),
                        key: key.clone(),
                        cache_status: CacheStatus::Updating,
                        store_response: true,
                        _fill_guard: Some(fill_guard),
                        cached_entry: Some(cached_entry.clone()),
                        cached_response_head,
                        revalidating: true,
                        request_forces_revalidation,
                        read_cached_body,
                    });
                    let Some(response) = self
                        .stale_response_from_entry(
                            &zone,
                            &key,
                            &cached_entry,
                            &request,
                            policy,
                            read_cached_body,
                            CacheStatus::Updating,
                        )
                        .await
                    else {
                        continue;
                    };
                    return CacheLookup::Updating(response, context);
                }
                LookupDecision::Miss {
                    key,
                    base_key: context_base_key,
                    cached_entry,
                    fill_guard,
                    cache_status,
                } => {
                    let cached_response_head = if let Some(entry) = &cached_entry {
                        match self.load_lookup_response_head(&zone, &key, entry).await {
                            Some(response_head) => Some(response_head),
                            None => {
                                drop(fill_guard);
                                continue;
                            }
                        }
                    } else {
                        None
                    };
                    if cache_status == CacheStatus::Miss {
                        zone.record_miss();
                    } else if cache_status == CacheStatus::Expired {
                        zone.record_expired();
                    }

                    return CacheLookup::Miss(Box::new(CacheStoreContext {
                        zone,
                        policy: policy.clone(),
                        request: request.clone(),
                        base_key: context_base_key,
                        key,
                        cache_status,
                        store_response: request.method == Method::GET
                            || (request.method == Method::HEAD && policy.convert_head),
                        _fill_guard: fill_guard,
                        cached_entry,
                        cached_response_head,
                        revalidating: cache_status == CacheStatus::Revalidated,
                        request_forces_revalidation,
                        read_cached_body,
                    }));
                }
                LookupDecision::ReadWhileFillLocal { state } => {
                    match super::fill::build_inflight_fill_response(
                        state,
                        &request,
                        policy,
                        read_cached_body,
                    ) {
                        Ok(response) => {
                            zone.record_miss();
                            return CacheLookup::Hit(with_cache_status(
                                response,
                                CacheStatus::Miss,
                            ));
                        }
                        Err(error) => {
                            tracing::warn!(
                                zone = %zone.config.name,
                                request_uri = %request.request_uri(),
                                %error,
                                "failed to serve response from in-flight cache fill"
                            );
                            tokio::task::yield_now().await;
                            continue;
                        }
                    }
                }
                LookupDecision::ReadWhileFillExternal { state } => {
                    match super::fill::build_external_inflight_fill_response(
                        state,
                        &request,
                        policy,
                        read_cached_body,
                    ) {
                        Ok(response) => {
                            zone.record_miss();
                            return CacheLookup::Hit(with_cache_status(
                                response,
                                CacheStatus::Miss,
                            ));
                        }
                        Err(error) => {
                            tracing::warn!(
                                zone = %zone.config.name,
                                request_uri = %request.request_uri(),
                                %error,
                                "failed to serve response from external in-flight cache fill"
                            );
                            tokio::task::yield_now().await;
                            continue;
                        }
                    }
                }
            }
        }
    }
}
