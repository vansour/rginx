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
                let index = load_index_from_disk(zone.as_ref()).map_err(|error| {
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
                        io_lock: AsyncMutex::new(()),
                        fill_locks: Arc::new(Mutex::new(HashMap::new())),
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

        let key = render_cache_key(
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
            match self.lookup_decision(&zone, &key, now, request_forces_revalidation) {
                LookupDecision::FreshHit(entry) => {
                    let cached_response = {
                        let _io_guard = zone.io_lock.lock().await;
                        read_cached_response(&zone, &key, &entry, read_cached_body).await
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
                            remove_index_entry(&zone, &key);
                            remove_cache_files_if_unindexed(&zone, &key, &entry.hash).await;
                        }
                    }
                }
                LookupDecision::StaleWhileRevalidate(entry) => {
                    match self
                        .stale_response_from_entry(&zone, &key, &entry, read_cached_body)
                        .await
                    {
                        Some(response) => return CacheLookup::Hit(response),
                        None => continue,
                    }
                }
                LookupDecision::Wait(waiter) => waiter.await,
                LookupDecision::Miss {
                    cached_entry,
                    fill_guard,
                    cache_status,
                    allow_stale_on_error,
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
                        key,
                        cache_status,
                        store_response: request.method == Method::GET,
                        _fill_guard: Some(fill_guard),
                        cached_entry,
                        cached_metadata,
                        allow_stale_on_error,
                        revalidating: cache_status == CacheStatus::Revalidated,
                        conditional_headers,
                        read_cached_body,
                    }));
                }
            }
        }
    }

    pub(crate) async fn store_response(
        &self,
        context: CacheStoreContext,
        response: HttpResponse,
    ) -> HttpResponse {
        let status = context.cache_status;
        let response = if context.store_response {
            match store_response(context, response).await {
                Ok(response) => response,
                Err(error) => {
                    tracing::warn!(%error, "failed to store cached response");
                    crate::handler::text_response(
                        StatusCode::BAD_GATEWAY,
                        "text/plain; charset=utf-8",
                        format!("failed to read upstream response while caching: {error}\n"),
                    )
                }
            }
        } else {
            response
        };

        with_cache_status(response, status)
    }

    pub(crate) async fn complete_not_modified(
        &self,
        context: CacheStoreContext,
        response: HttpResponse,
    ) -> std::result::Result<HttpResponse, CacheStoreError> {
        refresh_not_modified_response(context, response).await
    }

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
