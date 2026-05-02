use super::*;
use tokio::io::AsyncWriteExt;

pub(super) async fn store_empty_streaming_response(
    context: &CacheStoreContext,
    plan: EmptyStreamingCachePlan,
) {
    let metadata = cache_metadata(
        plan.final_key.clone(),
        plan.status,
        &plan.headers,
        cache_metadata_input(&context.base_key, plan.vary.clone(), plan.now, &plan.freshness, 0),
    );
    let removed_hashes = {
        let _io_guard = context.zone.io_write(&plan.hash).await;
        match write_cache_entry(&plan.paths, &metadata, &[]).await {
            Ok(()) => {
                context.zone.record_write_success();
                if context.revalidating {
                    context.zone.record_revalidated();
                }
                update_index_after_store(
                    &context.zone,
                    plan.final_key,
                    CacheIndexEntry {
                        hash: plan.hash,
                        base_key: context.base_key.clone(),
                        vary: plan.vary,
                        body_size_bytes: 0,
                        expires_at_unix_ms: metadata.expires_at_unix_ms,
                        stale_if_error_until_unix_ms: metadata.stale_if_error_until_unix_ms,
                        stale_while_revalidate_until_unix_ms: metadata
                            .stale_while_revalidate_until_unix_ms,
                        requires_revalidation: metadata.requires_revalidation,
                        must_revalidate: metadata.must_revalidate,
                        last_access_unix_ms: plan.now,
                    },
                    plan.replaced_entry,
                )
                .await
            }
            Err(error) => {
                tracing::warn!(
                    zone = %context.zone.config.name,
                    key_hash = %plan.hash,
                    %error,
                    "failed to write empty cache entry"
                );
                context.zone.record_write_error();
                std::collections::BTreeSet::new()
            }
        }
    };
    for removed_hash in removed_hashes {
        remove_cache_files_if_unreferenced(context.zone.as_ref(), &removed_hash).await;
    }
}

pub(super) fn record_streaming_cache_write_error(
    plan: &StreamingCachePlan,
    error: &std::io::Error,
) {
    tracing::warn!(
        zone = %plan.zone.config.name,
        key_hash = %plan.hash,
        %error,
        "failed to stream cache entry body"
    );
    plan.zone.record_write_error();
}

pub(super) fn start_streaming_cache_finalize(
    cache: ActiveStreamingCache,
) -> StreamingCacheFinalize {
    Box::pin(complete_streaming_cache(cache))
}

pub(super) fn abandon_streaming_cache(cache: ActiveStreamingCache) {
    spawn_cache_task(async move {
        cleanup_streaming_cache(cache).await;
    });
}

fn spawn_cache_task(task: impl std::future::Future<Output = ()> + Send + 'static) {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(task);
    }
}

async fn complete_streaming_cache(mut cache: ActiveStreamingCache) {
    if let Err(error) = cache.file.flush().await {
        record_streaming_cache_write_error(&cache.plan, &error);
        drop(cache.file);
        let _ = fs::remove_file(&cache.plan.body_tmp).await;
        return;
    }
    drop(cache.file);

    let vary = cache.plan.vary.clone();
    let metadata = cache_metadata(
        cache.plan.final_key.clone(),
        cache.plan.status,
        &cache.plan.headers,
        cache_metadata_input(
            &cache.plan.base_key,
            vary.clone(),
            cache.plan.now,
            &cache.plan.freshness,
            cache.plan.body_size_bytes,
        ),
    );
    let removed_hashes = {
        let _io_guard = cache.plan.zone.io_write(&cache.plan.hash).await;
        match commit_cache_entry_temp_body(&cache.plan.paths, &metadata, &cache.plan.body_tmp).await
        {
            Ok(()) => {
                cache.plan.zone.record_write_success();
                if cache.plan.revalidating {
                    cache.plan.zone.record_revalidated();
                }
                update_index_after_store(
                    &cache.plan.zone,
                    cache.plan.final_key.clone(),
                    CacheIndexEntry {
                        hash: cache.plan.hash.clone(),
                        base_key: cache.plan.base_key.clone(),
                        vary,
                        body_size_bytes: metadata.body_size_bytes,
                        expires_at_unix_ms: metadata.expires_at_unix_ms,
                        stale_if_error_until_unix_ms: metadata.stale_if_error_until_unix_ms,
                        stale_while_revalidate_until_unix_ms: metadata
                            .stale_while_revalidate_until_unix_ms,
                        requires_revalidation: metadata.requires_revalidation,
                        must_revalidate: metadata.must_revalidate,
                        last_access_unix_ms: cache.plan.now,
                    },
                    cache.plan.replaced_entry.clone(),
                )
                .await
            }
            Err(error) => {
                record_streaming_cache_write_error(&cache.plan, &error);
                std::collections::BTreeSet::new()
            }
        }
    };
    for removed_hash in removed_hashes {
        remove_cache_files_if_unreferenced(cache.plan.zone.as_ref(), &removed_hash).await;
    }
}

async fn cleanup_streaming_cache(mut cache: ActiveStreamingCache) {
    let _ = cache.file.flush().await;
    drop(cache.file);
    let _ = fs::remove_file(&cache.plan.body_tmp).await;
}
