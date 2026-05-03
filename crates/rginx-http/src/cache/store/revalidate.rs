use super::*;

pub(in crate::cache) async fn refresh_not_modified_response(
    context: CacheStoreContext,
    response: HttpResponse,
) -> std::result::Result<HttpResponse, CacheStoreError> {
    let Some(cached_entry) = context.cached_entry.clone() else {
        context.zone.record_write_error();
        return Err(CacheStoreError {
            source: Box::new(std::io::Error::other("missing cached entry for 304 revalidation")),
        });
    };
    let Some(cached_response_head) = context.cached_response_head.clone() else {
        context.zone.record_write_error();
        return Err(CacheStoreError {
            source: Box::new(std::io::Error::other(
                "missing cached response head for 304 revalidation",
            )),
        });
    };
    let cached_metadata = cached_response_head.metadata.as_ref();

    let cached_headers = cached_response_head.headers.clone();
    let merged_headers = merge_not_modified_headers(&cached_headers, response.headers());
    let now = unix_time_ms(SystemTime::now());
    let cached_status = cached_response_head.status;
    let paths = cache_paths_for_zone(context.zone.config.as_ref(), &cached_entry.hash);
    if response_no_cache(&context, cached_status) {
        let freshness = response_freshness(&context, cached_status, &merged_headers);
        let (final_key, vary, tags) =
            cache_final_key_for_response(&context, &context.request, &merged_headers);
        let mut metadata_input = cache_metadata_input(
            &context.base_key,
            vary.clone(),
            tags.clone(),
            now,
            context.policy.grace,
            context.policy.keep,
            &freshness,
            cached_metadata.body_size_bytes,
        );
        metadata_input.requires_revalidation = true;
        let response_metadata =
            cache_metadata(final_key.clone(), cached_status, &merged_headers, metadata_input);
        let (refreshed, removed_hashes) = {
            let response_head = Arc::new(
                prepare_cached_response_head(&cached_entry.hash, response_metadata)
                    .map_err(|error| CacheStoreError { source: Box::new(error) })?,
            );
            let _io_guard = context.zone.io_write(&cached_entry.hash).await;
            write_cache_metadata(&paths, response_head.metadata.as_ref())
                .await
                .map_err(|error| CacheStoreError { source: Box::new(error) })?;
            context.zone.record_write_success();
            context.zone.record_revalidated();
            let removed_hashes = update_index_after_store(
                &context.zone,
                final_key.clone(),
                CacheIndexEntry {
                    kind: response_head.metadata.kind,
                    hash: cached_entry.hash.clone(),
                    base_key: context.base_key.clone(),
                    stored_at_unix_ms: response_head.metadata.stored_at_unix_ms,
                    vary,
                    tags,
                    body_size_bytes: response_head.metadata.body_size_bytes,
                    expires_at_unix_ms: response_head.metadata.expires_at_unix_ms,
                    grace_until_unix_ms: response_head.metadata.grace_until_unix_ms,
                    keep_until_unix_ms: response_head.metadata.keep_until_unix_ms,
                    stale_if_error_until_unix_ms: response_head
                        .metadata
                        .stale_if_error_until_unix_ms,
                    stale_while_revalidate_until_unix_ms: response_head
                        .metadata
                        .stale_while_revalidate_until_unix_ms,
                    requires_revalidation: response_head.metadata.requires_revalidation,
                    must_revalidate: response_head.metadata.must_revalidate,
                    last_access_unix_ms: now,
                },
                (final_key != context.key).then_some((context.key.clone(), cached_entry.clone())),
            )
            .await;
            context.zone.store_prepared_response_head(&final_key, now, response_head.clone());
            let refreshed = build_cached_response_for_request(
                &paths.body,
                response_head.as_ref(),
                &context.request,
                &context.policy,
                context.read_cached_body,
            )
            .await
            .map_err(|error| CacheStoreError { source: Box::new(error) })?;
            (refreshed, removed_hashes)
        };
        for removed_hash in removed_hashes {
            remove_cache_files_if_unreferenced(context.zone.as_ref(), &removed_hash).await;
        }
        return Ok(with_cache_status(refreshed, CacheStatus::Revalidated));
    }

    let freshness = response_freshness(&context, cached_status, &merged_headers);
    let (final_key, vary, tags) =
        cache_final_key_for_response(&context, &context.request, &merged_headers);
    let metadata = cache_metadata(
        final_key.clone(),
        cached_status,
        &merged_headers,
        cache_metadata_input(
            &context.base_key,
            vary.clone(),
            tags.clone(),
            now,
            context.policy.grace,
            context.policy.keep,
            &freshness,
            cached_metadata.body_size_bytes,
        ),
    );
    if !response_is_storable_with_size(
        &context,
        cached_status,
        &merged_headers,
        ResponseBodySize::exact(cached_metadata.body_size_bytes),
    ) || !freshness_is_cacheable(&freshness)
        || final_key != context.key
    {
        let response_head = prepare_cached_response_head(&cached_entry.hash, metadata)
            .map_err(|error| CacheStoreError { source: Box::new(error) })?;
        let refreshed = {
            let _io_guard = context.zone.io_write(&cached_entry.hash).await;
            build_cached_response_for_request(
                &paths.body,
                &response_head,
                &context.request,
                &context.policy,
                context.read_cached_body,
            )
            .await
            .map_err(|error| CacheStoreError { source: Box::new(error) })?
        };
        let remember_pass = final_key == context.key
            && (should_remember_hit_for_pass(&context, &merged_headers)
                || !freshness_is_cacheable(&freshness));
        let removed_hashes = if remember_pass {
            remember_hit_for_pass(&context, &merged_headers, now).await
        } else {
            std::collections::BTreeSet::new()
        };
        if removed_hashes.is_empty() {
            if let Some(removed) =
                remove_zone_index_entry_if_matches(&context.zone, &context.key, &cached_entry).await
                && removed.delete_files
            {
                remove_cache_files_locked(context.zone.config.as_ref(), &removed.hash).await;
            }
        } else {
            for removed_hash in removed_hashes {
                remove_cache_files_if_unreferenced(context.zone.as_ref(), &removed_hash).await;
            }
        }
        context.zone.record_revalidated();
        return Ok(with_cache_status(refreshed, CacheStatus::Revalidated));
    }
    let (refreshed, removed_hashes) = {
        let response_head = Arc::new(
            prepare_cached_response_head(&cached_entry.hash, metadata)
                .map_err(|error| CacheStoreError { source: Box::new(error) })?,
        );
        let _io_guard = context.zone.io_write(&cached_entry.hash).await;
        write_cache_metadata(&paths, response_head.metadata.as_ref())
            .await
            .map_err(|error| CacheStoreError { source: Box::new(error) })?;
        context.zone.record_write_success();
        context.zone.record_revalidated();
        let key = context.key.clone();
        let removed_hashes = update_index_after_store(
            &context.zone,
            key.clone(),
            CacheIndexEntry {
                kind: response_head.metadata.kind,
                hash: cached_entry.hash.clone(),
                base_key: context.base_key.clone(),
                stored_at_unix_ms: response_head.metadata.stored_at_unix_ms,
                vary,
                tags,
                body_size_bytes: response_head.metadata.body_size_bytes,
                expires_at_unix_ms: response_head.metadata.expires_at_unix_ms,
                grace_until_unix_ms: response_head.metadata.grace_until_unix_ms,
                keep_until_unix_ms: response_head.metadata.keep_until_unix_ms,
                stale_if_error_until_unix_ms: response_head.metadata.stale_if_error_until_unix_ms,
                stale_while_revalidate_until_unix_ms: response_head
                    .metadata
                    .stale_while_revalidate_until_unix_ms,
                requires_revalidation: response_head.metadata.requires_revalidation,
                must_revalidate: response_head.metadata.must_revalidate,
                last_access_unix_ms: now,
            },
            None,
        )
        .await;
        context.zone.store_prepared_response_head(&key, now, response_head.clone());
        let refreshed = build_cached_response_for_request(
            &paths.body,
            response_head.as_ref(),
            &context.request,
            &context.policy,
            context.read_cached_body,
        )
        .await
        .map_err(|error| CacheStoreError { source: Box::new(error) })?;
        (refreshed, removed_hashes)
    };
    for removed_hash in removed_hashes {
        remove_cache_files_if_unreferenced(context.zone.as_ref(), &removed_hash).await;
    }
    Ok(with_cache_status(refreshed, CacheStatus::Revalidated))
}
