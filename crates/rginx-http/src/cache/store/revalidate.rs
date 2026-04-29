use http::StatusCode;

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
    let Some(cached_metadata) = context.cached_metadata.clone() else {
        context.zone.record_write_error();
        return Err(CacheStoreError {
            source: Box::new(std::io::Error::other("missing cached metadata for 304 revalidation")),
        });
    };

    let cached_headers = cached_metadata
        .headers_map()
        .map_err(|error| CacheStoreError { source: Box::new(error) })?;
    let merged_headers = merge_not_modified_headers(&cached_headers, response.headers());
    let now = unix_time_ms(SystemTime::now());
    let cached_status = StatusCode::from_u16(cached_metadata.status).unwrap_or(StatusCode::OK);
    let paths = cache_paths_for_zone(context.zone.config.as_ref(), &cached_entry.hash);
    if response_no_cache(&context, cached_status) {
        let response_metadata = cache_metadata(
            cached_metadata.key.clone(),
            cached_status,
            &merged_headers,
            CacheMetadataInput {
                base_key: cached_metadata.base_key.clone(),
                vary: cached_entry.vary.clone(),
                stored_at_unix_ms: cached_metadata.stored_at_unix_ms,
                expires_at_unix_ms: cached_metadata.expires_at_unix_ms,
                stale_if_error_until_unix_ms: cached_metadata.stale_if_error_until_unix_ms,
                stale_while_revalidate_until_unix_ms: cached_metadata
                    .stale_while_revalidate_until_unix_ms,
                must_revalidate: cached_metadata.must_revalidate,
                body_size_bytes: cached_metadata.body_size_bytes,
            },
        );
        let refreshed = {
            let _io_guard = context.zone.io_lock.lock().await;
            build_cached_response_for_request(
                &paths.body,
                &response_metadata,
                &context.request,
                &context.policy,
                context.read_cached_body,
            )
            .await
        };
        return refreshed
            .map(|response| with_cache_status(response, CacheStatus::Revalidated))
            .map_err(|error| CacheStoreError { source: Box::new(error) });
    }

    let freshness = response_freshness(&context, cached_status, &merged_headers);
    let vary = cache_vary_values(&context, &context.request, &merged_headers);
    let final_key = cache_variant_key(&context.base_key, &vary);
    let metadata = cache_metadata(
        final_key.clone(),
        cached_status,
        &merged_headers,
        cache_metadata_input(
            &context.base_key,
            vary.clone(),
            now,
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
        let refreshed = {
            let _io_guard = context.zone.io_lock.lock().await;
            build_cached_response_for_request(
                &paths.body,
                &metadata,
                &context.request,
                &context.policy,
                context.read_cached_body,
            )
            .await
            .map_err(|error| CacheStoreError { source: Box::new(error) })
        };
        remove_zone_index_entry(&context.zone, &context.key).await;
        {
            let _io_guard = context.zone.io_lock.lock().await;
            let _ = fs::remove_file(&paths.metadata).await;
            let _ = fs::remove_file(&paths.body).await;
        }
        context.zone.record_revalidated();
        return refreshed.map(|response| with_cache_status(response, CacheStatus::Revalidated));
    }
    {
        let _io_guard = context.zone.io_lock.lock().await;
        write_cache_metadata(&paths, &metadata)
            .await
            .map_err(|error| CacheStoreError { source: Box::new(error) })?;
    }
    context.zone.record_write_success();
    context.zone.record_revalidated();
    update_index_after_store(
        &context.zone,
        context.key,
        CacheIndexEntry {
            hash: cached_entry.hash.clone(),
            base_key: context.base_key.clone(),
            vary,
            body_size_bytes: metadata.body_size_bytes,
            expires_at_unix_ms: metadata.expires_at_unix_ms,
            stale_if_error_until_unix_ms: metadata.stale_if_error_until_unix_ms,
            stale_while_revalidate_until_unix_ms: metadata.stale_while_revalidate_until_unix_ms,
            must_revalidate: metadata.must_revalidate,
            last_access_unix_ms: now,
        },
        None,
    )
    .await;

    let refreshed = {
        let _io_guard = context.zone.io_lock.lock().await;
        build_cached_response_for_request(
            &paths.body,
            &metadata,
            &context.request,
            &context.policy,
            context.read_cached_body,
        )
        .await
        .map_err(|error| CacheStoreError { source: Box::new(error) })?
    };
    Ok(with_cache_status(refreshed, CacheStatus::Revalidated))
}
