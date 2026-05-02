use super::*;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

pub(super) fn spawn_streaming_cache_writer(
    plan: StreamingCachePlan,
    file: File,
    queue_depth: usize,
) -> StreamingCacheWriter {
    let fill_state = plan.fill_state.clone();
    let (tx, rx) = mpsc::channel(queue_depth.max(1));
    spawn_cache_task(async move {
        complete_streaming_cache(plan, file, rx).await;
    });
    StreamingCacheWriter::new(tx, fill_state)
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

fn spawn_cache_task(task: impl std::future::Future<Output = ()> + Send + 'static) {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(task);
        return;
    }
    std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return;
        };
        runtime.block_on(task);
    });
}

async fn finalize_streaming_cache(plan: StreamingCachePlan, body_size_bytes: usize) {
    if body_size_bytes > plan.max_entry_bytes {
        let _ = fs::remove_file(&plan.body_tmp).await;
        return;
    }

    if let Some(expected_body_bytes) = plan.expected_body_bytes
        && expected_body_bytes != body_size_bytes
    {
        record_streaming_cache_write_error(
            &plan,
            &std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "streamed body length `{body_size_bytes}` does not match exact size hint `{expected_body_bytes}`"
                ),
            ),
        );
        if let Some(fill_state) = plan.fill_state.as_ref() {
            fill_state.fail(format!(
                "streamed body length `{body_size_bytes}` does not match exact size hint `{expected_body_bytes}`"
            ));
        }
        let _ = fs::remove_file(&plan.body_tmp).await;
        return;
    }

    let vary = plan.vary.clone();
    let metadata = cache_metadata(
        plan.final_key.clone(),
        plan.status,
        &plan.headers,
        cache_metadata_input(
            &plan.base_key,
            vary.clone(),
            plan.now,
            &plan.freshness,
            body_size_bytes,
        ),
    );
    let removed_hashes = {
        let _io_guard = plan.zone.io_write(&plan.hash).await;
        match commit_cache_entry_temp_body(&plan.paths, &metadata, &plan.body_tmp).await {
            Ok(()) => {
                plan.zone.record_write_success();
                if plan.revalidating {
                    plan.zone.record_revalidated();
                }
                update_index_after_store(
                    &plan.zone,
                    plan.final_key.clone(),
                    CacheIndexEntry {
                        hash: plan.hash.clone(),
                        base_key: plan.base_key.clone(),
                        vary,
                        body_size_bytes: metadata.body_size_bytes,
                        expires_at_unix_ms: metadata.expires_at_unix_ms,
                        stale_if_error_until_unix_ms: metadata.stale_if_error_until_unix_ms,
                        stale_while_revalidate_until_unix_ms: metadata
                            .stale_while_revalidate_until_unix_ms,
                        requires_revalidation: metadata.requires_revalidation,
                        must_revalidate: metadata.must_revalidate,
                        last_access_unix_ms: plan.now,
                    },
                    plan.replaced_entry.clone(),
                )
                .await
            }
            Err(error) => {
                record_streaming_cache_write_error(&plan, &error);
                std::collections::BTreeSet::new()
            }
        }
    };
    for removed_hash in removed_hashes {
        remove_cache_files_if_unreferenced(plan.zone.as_ref(), &removed_hash).await;
    }
}

async fn complete_streaming_cache(
    plan: StreamingCachePlan,
    mut file: File,
    mut rx: mpsc::Receiver<StreamingCacheWriteMessage>,
) {
    let mut body_size_bytes = 0usize;
    while let Some(message) = rx.recv().await {
        match message {
            StreamingCacheWriteMessage::Data(bytes) => {
                if let Err(error) = file.write_all(&bytes).await {
                    record_streaming_cache_write_error(&plan, &error);
                    if let Some(fill_state) = plan.fill_state.as_ref() {
                        fill_state.fail(&error);
                    }
                    drop(file);
                    let _ = fs::remove_file(&plan.body_tmp).await;
                    return;
                }
                body_size_bytes = body_size_bytes.saturating_add(bytes.len());
                if let Some(fill_state) = plan.fill_state.as_ref() {
                    fill_state.record_bytes_written(body_size_bytes);
                }
            }
            StreamingCacheWriteMessage::Finish { trailers } => {
                if let Err(error) = file.flush().await {
                    record_streaming_cache_write_error(&plan, &error);
                    if let Some(fill_state) = plan.fill_state.as_ref() {
                        fill_state.fail(&error);
                    }
                    drop(file);
                    let _ = fs::remove_file(&plan.body_tmp).await;
                    return;
                }
                if let Some(fill_state) = plan.fill_state.as_ref() {
                    fill_state.finish(trailers);
                }
                drop(file);
                finalize_streaming_cache(plan, body_size_bytes).await;
                return;
            }
            StreamingCacheWriteMessage::Abort => {
                let _ = file.flush().await;
                drop(file);
                let _ = fs::remove_file(&plan.body_tmp).await;
                return;
            }
        }
    }

    if let Some(fill_state) = plan.fill_state.as_ref() {
        fill_state.fail("streaming cache writer channel closed before end-of-stream");
    }
    let _ = file.flush().await;
    drop(file);
    let _ = fs::remove_file(&plan.body_tmp).await;
}
