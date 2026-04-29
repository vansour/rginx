use super::*;

mod control;
mod response;

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
                let (index, shared_index_store, shared_index_generation) =
                    bootstrap_shared_index(zone.as_ref()).map_err(|error| {
                        Error::Server(format!(
                            "failed to load cache zone `{name}` index from `{}`: {error}",
                            zone.path.display()
                        ))
                    })?;
                Ok((
                    name.clone(),
                    Arc::new(CacheZoneRuntime {
                        config: zone.clone(),
                        index: Mutex::new(index),
                        io_locks: CacheIoLockPool::new(),
                        shared_index_sync_lock: AsyncMutex::new(()),
                        shared_index_store,
                        fill_locks: Arc::new(Mutex::new(HashMap::new())),
                        fill_lock_generation: AtomicU64::new(0),
                        last_inactive_cleanup_unix_ms: AtomicU64::new(0),
                        shared_index_generation: AtomicU64::new(shared_index_generation),
                        stats: CacheZoneStats::default(),
                        change_notifier: change_notifier.clone(),
                    }),
                ))
            })
            .collect::<Result<HashMap<_, _>>>()?;

        Ok(Self { zones: Arc::new(zones) })
    }

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
                LookupDecision::FreshHit { key, entry } => {
                    let cached_response = {
                        let _io_guard = zone.io_read(&entry.hash).await;
                        read_cached_response_for_request(
                            &zone,
                            &key,
                            &entry,
                            &request,
                            policy,
                            read_cached_body,
                        )
                        .await
                    };
                    match cached_response {
                        Ok(mut response) => {
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
                        cached_metadata: None,
                        revalidating: false,
                        conditional_headers: None,
                        read_cached_body,
                    }));
                }
                LookupDecision::BackgroundUpdate { key, cached_entry, fill_guard } => {
                    let (cached_metadata, conditional_headers) =
                        match self.load_lookup_metadata(&zone, &key, &cached_entry).await {
                            Some((metadata, conditional_headers)) => {
                                (Some(metadata), conditional_headers)
                            }
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
                        cached_metadata,
                        revalidating: true,
                        conditional_headers,
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
                    let (cached_metadata, conditional_headers) = if let Some(entry) = &cached_entry
                    {
                        match self.load_lookup_metadata(&zone, &key, entry).await {
                            Some((metadata, conditional_headers)) => {
                                (Some(metadata), conditional_headers)
                            }
                            None => {
                                drop(fill_guard);
                                continue;
                            }
                        }
                    } else {
                        (None, None)
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
                        cached_metadata,
                        revalidating: cache_status == CacheStatus::Revalidated,
                        conditional_headers,
                        read_cached_body,
                    }));
                }
            }
        }
    }
}
