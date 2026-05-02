use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use http::StatusCode;
use http::header::HeaderMap;
use http_body_util::BodyExt;
use hyper::body::{Body as _, Frame, SizeHint};
use tokio::fs::{self, File};
use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::handler::{BoxError, HttpBody, HttpResponse, boxed_body, full_body};

use super::entry::{
    CacheMetadataInput, CachePaths, build_cached_response_for_request, cache_entry_temp_body_path,
    cache_key_hash, cache_metadata, cache_paths_for_zone, cache_variant_key,
    commit_cache_entry_temp_body, finalize_response_for_request, unix_time_ms, write_cache_entry,
    write_cache_metadata,
};
use super::policy::{
    ResponseBodySize, ResponseFreshness, response_freshness, response_is_storable,
    response_is_storable_with_size, response_no_cache,
};
use super::{
    CacheIndex, CacheIndexEntry, CachePurgeResult, CacheStatus, CacheStoreContext,
    CacheZoneRuntime, remove_cache_files_if_unreferenced, remove_cache_files_locked,
    with_cache_status,
};

mod helpers;
mod maintenance;
mod revalidate;

use helpers::{
    cache_metadata_input, cache_vary_values, freshness_is_cacheable, merge_not_modified_headers,
};
pub(in crate::cache) use helpers::{purge_scope, purge_selector_matches};
pub(super) use maintenance::{
    cleanup_inactive_entries_in_zone, eviction_candidates, lock_index, purge_zone_entries,
    record_cache_admission_attempt, remove_zone_index_entry_if_matches,
};
pub(in crate::cache) use revalidate::refresh_not_modified_response;

pub(crate) struct CacheStoreError {
    source: Box<dyn std::error::Error + Send + Sync>,
}

struct StreamingCachePlan {
    zone: Arc<CacheZoneRuntime>,
    base_key: String,
    final_key: String,
    vary: Vec<super::CachedVaryHeaderValue>,
    status: StatusCode,
    headers: HeaderMap,
    freshness: ResponseFreshness,
    now: u64,
    hash: String,
    paths: CachePaths,
    body_tmp: PathBuf,
    body_size_bytes: usize,
    revalidating: bool,
    replaced_entry: Option<(String, CacheIndexEntry)>,
    _fill_guard: Option<super::CacheFillGuard>,
}

struct PendingCacheWrite {
    bytes: Bytes,
    written: usize,
}

struct EmptyStreamingCachePlan {
    final_key: String,
    vary: Vec<super::CachedVaryHeaderValue>,
    hash: String,
    paths: CachePaths,
    now: u64,
    replaced_entry: Option<(String, CacheIndexEntry)>,
    status: StatusCode,
    headers: HeaderMap,
    freshness: ResponseFreshness,
}

struct ActiveStreamingCache {
    file: File,
    pending_write: Option<PendingCacheWrite>,
    finalize_after_pending_frame: bool,
    plan: StreamingCachePlan,
}

type StreamingCacheFinalize = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

struct StreamingCacheBody {
    inner: HttpBody,
    size_hint: SizeHint,
    expected_body_bytes: Option<usize>,
    pending_frame: Option<Frame<Bytes>>,
    cache: Option<ActiveStreamingCache>,
    finalizing: Option<StreamingCacheFinalize>,
    done: bool,
}

impl std::fmt::Display for CacheStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.source)
    }
}

impl std::fmt::Debug for CacheStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("CacheStoreError").field("source", &self.source).finish()
    }
}

impl std::error::Error for CacheStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

impl PendingCacheWrite {
    fn new(bytes: Bytes) -> Self {
        Self { bytes, written: 0 }
    }

    fn remaining(&self) -> &[u8] {
        &self.bytes[self.written..]
    }
}

impl StreamingCacheBody {
    fn new(inner: HttpBody, size_hint: SizeHint, cache: ActiveStreamingCache) -> Self {
        Self {
            inner,
            expected_body_bytes: size_hint.exact().and_then(|exact| usize::try_from(exact).ok()),
            size_hint,
            pending_frame: None,
            cache: Some(cache),
            finalizing: None,
            done: false,
        }
    }
}

impl Drop for StreamingCacheBody {
    fn drop(&mut self) {
        if let Some(finalizing) = self.finalizing.take() {
            spawn_cache_task(finalizing);
            return;
        }
        if let Some(cache) = self.cache.take() {
            abandon_streaming_cache(cache);
        }
    }
}

impl hyper::body::Body for StreamingCacheBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();

        loop {
            if let Some(finalizing) = this.finalizing.as_mut() {
                match finalizing.as_mut().poll(cx) {
                    Poll::Ready(()) => {
                        this.finalizing = None;
                        if let Some(frame) = this.pending_frame.take() {
                            return Poll::Ready(Some(Ok(frame)));
                        }
                        return Poll::Ready(None);
                    }
                    Poll::Pending => return Poll::Pending,
                }
            }

            let mut should_return_pending_frame = false;
            let mut disable_cache: Option<(std::io::Error, bool)> = None;

            {
                if let Some(cache) = this.cache.as_mut()
                    && let Some(pending_write) = cache.pending_write.as_mut()
                {
                    match Pin::new(&mut cache.file).poll_write(cx, pending_write.remaining()) {
                        Poll::Ready(Ok(0)) => {
                            disable_cache = Some((
                                std::io::Error::new(
                                    std::io::ErrorKind::WriteZero,
                                    "cache body write returned zero bytes",
                                ),
                                true,
                            ));
                        }
                        Poll::Ready(Ok(written)) => {
                            pending_write.written += written;
                            if pending_write.written == pending_write.bytes.len() {
                                cache.plan.body_size_bytes = cache
                                    .plan
                                    .body_size_bytes
                                    .saturating_add(pending_write.bytes.len());
                                cache.pending_write = None;
                                if cache.finalize_after_pending_frame {
                                    cache.finalize_after_pending_frame = false;
                                    if let Some(cache) = this.cache.take() {
                                        this.finalizing =
                                            Some(start_streaming_cache_finalize(cache));
                                        continue;
                                    }
                                } else {
                                    should_return_pending_frame = true;
                                }
                            }
                        }
                        Poll::Ready(Err(error)) => {
                            disable_cache = Some((error, true));
                        }
                        Poll::Pending => return Poll::Pending,
                    }
                }
            }

            if let Some((error, record_write_error)) = disable_cache.take() {
                let frame =
                    this.pending_frame.take().expect("pending frame should accompany writes");
                if let Some(cache) = this.cache.take() {
                    if record_write_error {
                        record_streaming_cache_write_error(&cache.plan, &error);
                    }
                    abandon_streaming_cache(cache);
                }
                return Poll::Ready(Some(Ok(frame)));
            }

            if should_return_pending_frame {
                let frame = this.pending_frame.take().expect("pending frame should be available");
                return Poll::Ready(Some(Ok(frame)));
            }

            if this.done {
                return Poll::Ready(None);
            }

            match Pin::new(&mut this.inner).poll_frame(cx) {
                Poll::Ready(Some(Ok(frame))) => {
                    if let Some(data) = frame.data_ref() {
                        let data = data.clone();
                        let stream_completed = frame.is_trailers()
                            || this.inner.is_end_stream()
                            || this.expected_body_bytes.is_some_and(|expected| {
                                expected
                                    <= this.cache.as_ref().map_or(data.len(), |cache| {
                                        cache.plan.body_size_bytes.saturating_add(data.len())
                                    })
                            });
                        this.done = stream_completed;
                        let mut overflowed = false;
                        if let Some(cache) = this.cache.as_mut() {
                            if cache.plan.body_size_bytes.saturating_add(data.len())
                                > cache.plan.zone.config.max_entry_bytes
                            {
                                overflowed = true;
                            } else if !data.is_empty() {
                                this.pending_frame = Some(frame);
                                cache.pending_write = Some(PendingCacheWrite::new(data));
                                cache.finalize_after_pending_frame = stream_completed;
                                continue;
                            } else if stream_completed {
                                this.pending_frame = Some(frame);
                                if let Some(cache) = this.cache.take() {
                                    this.finalizing = Some(start_streaming_cache_finalize(cache));
                                    continue;
                                }
                                let frame = this
                                    .pending_frame
                                    .take()
                                    .expect("terminal empty frame should be available");
                                return Poll::Ready(Some(Ok(frame)));
                            }
                        }

                        if overflowed && let Some(cache) = this.cache.take() {
                            abandon_streaming_cache(cache);
                        }
                        return Poll::Ready(Some(Ok(frame)));
                    }

                    let stream_completed = frame.is_trailers() || this.inner.is_end_stream();
                    this.done = stream_completed;

                    if stream_completed && let Some(cache) = this.cache.take() {
                        this.pending_frame = Some(frame);
                        this.finalizing = Some(start_streaming_cache_finalize(cache));
                        continue;
                    }
                    return Poll::Ready(Some(Ok(frame)));
                }
                Poll::Ready(Some(Err(error))) => {
                    this.done = true;
                    if let Some(cache) = this.cache.take() {
                        cache.plan.zone.record_write_error();
                        abandon_streaming_cache(cache);
                    }
                    return Poll::Ready(Some(Err(error)));
                }
                Poll::Ready(None) => {
                    this.done = true;
                    if let Some(cache) = this.cache.take() {
                        this.finalizing = Some(start_streaming_cache_finalize(cache));
                        continue;
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.pending_frame.is_none()
            && self.cache.is_none()
            && self.finalizing.is_none()
            && (self.done || self.inner.is_end_stream())
    }

    fn size_hint(&self) -> SizeHint {
        self.size_hint.clone()
    }
}

pub(super) async fn store_response(
    context: CacheStoreContext,
    response: HttpResponse,
) -> std::result::Result<HttpResponse, CacheStoreError> {
    let needs_downstream_range_trim =
        super::request::cacheable_range_request(&context.request, &context.policy)
            .is_some_and(|range| range.needs_downstream_trimming());
    let storable = response_is_storable(&context, &response);
    let no_cache = response_no_cache(&context, response.status());
    if !needs_downstream_range_trim && !storable {
        return Ok(response);
    }
    if !needs_downstream_range_trim && no_cache {
        return Ok(response);
    }

    let (parts, body) = response.into_parts();
    if needs_downstream_range_trim {
        return store_buffered_response(context, parts, body, storable, no_cache).await;
    }

    Ok(store_streaming_response(context, parts, body).await)
}

async fn store_buffered_response(
    context: CacheStoreContext,
    parts: http::response::Parts,
    body: HttpBody,
    storable: bool,
    no_cache: bool,
) -> std::result::Result<HttpResponse, CacheStoreError> {
    let collected = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => {
            context.zone.record_write_error();
            return Err(CacheStoreError { source: error });
        }
    };
    let downstream_response =
        || finalize_downstream_response(&context, parts.status, &parts.headers, collected.as_ref());

    if !storable || no_cache || collected.len() > context.zone.config.max_entry_bytes {
        return downstream_response();
    }

    let now = unix_time_ms(SystemTime::now());
    let freshness = response_freshness(&context, parts.status, &parts.headers);
    if !freshness_is_cacheable(&freshness) {
        return downstream_response();
    }

    let vary = cache_vary_values(&context, &context.request, &parts.headers);
    let final_key = cache_variant_key(&context.base_key, &vary);
    let admission =
        record_cache_admission_attempt(&context.zone, &final_key, context.policy.min_uses).await;
    if !admission.admitted {
        return downstream_response();
    }
    let metadata = cache_metadata(
        final_key.clone(),
        parts.status,
        &parts.headers,
        cache_metadata_input(&context.base_key, vary.clone(), now, &freshness, collected.len()),
    );
    let hash = context
        .cached_entry
        .as_ref()
        .filter(|_| context.key == final_key)
        .map(|entry| entry.hash.clone())
        .unwrap_or_else(|| cache_key_hash(&final_key));
    let paths = cache_paths_for_zone(context.zone.config.as_ref(), &hash);
    let removed_hashes = {
        let _io_guard = context.zone.io_write(&hash).await;

        if let Err(error) = write_cache_entry(&paths, &metadata, &collected).await {
            tracing::warn!(
                zone = %context.zone.config.name,
                key_hash = %hash,
                %error,
                "failed to write cache entry"
            );
            context.zone.record_write_error();
            std::collections::BTreeSet::new()
        } else {
            context.zone.record_write_success();
            if context.revalidating {
                context.zone.record_revalidated();
            }
            update_index_after_store(
                &context.zone,
                final_key.clone(),
                CacheIndexEntry {
                    hash,
                    base_key: context.base_key.clone(),
                    vary,
                    body_size_bytes: metadata.body_size_bytes,
                    expires_at_unix_ms: metadata.expires_at_unix_ms,
                    stale_if_error_until_unix_ms: metadata.stale_if_error_until_unix_ms,
                    stale_while_revalidate_until_unix_ms: metadata
                        .stale_while_revalidate_until_unix_ms,
                    requires_revalidation: metadata.requires_revalidation,
                    must_revalidate: metadata.must_revalidate,
                    last_access_unix_ms: now,
                },
                context
                    .cached_entry
                    .as_ref()
                    .filter(|_| context.key != final_key)
                    .map(|entry| (context.key.clone(), entry.clone())),
            )
            .await
        }
    };
    for removed_hash in removed_hashes {
        remove_cache_files_if_unreferenced(context.zone.as_ref(), &removed_hash).await;
    }

    downstream_response()
}

async fn store_streaming_response(
    context: CacheStoreContext,
    parts: http::response::Parts,
    body: HttpBody,
) -> HttpResponse {
    let freshness = response_freshness(&context, parts.status, &parts.headers);
    if !freshness_is_cacheable(&freshness) {
        return passthrough_response(parts, body);
    }

    let vary = cache_vary_values(&context, &context.request, &parts.headers);
    let final_key = cache_variant_key(&context.base_key, &vary);
    let admission =
        record_cache_admission_attempt(&context.zone, &final_key, context.policy.min_uses).await;
    if !admission.admitted {
        return passthrough_response(parts, body);
    }

    let now = unix_time_ms(SystemTime::now());
    let hash = context
        .cached_entry
        .as_ref()
        .filter(|_| context.key == final_key)
        .map(|entry| entry.hash.clone())
        .unwrap_or_else(|| cache_key_hash(&final_key));
    let paths = cache_paths_for_zone(context.zone.config.as_ref(), &hash);
    let replaced_entry = context
        .cached_entry
        .as_ref()
        .filter(|_| context.key != final_key)
        .map(|entry| (context.key.clone(), entry.clone()));
    let size_hint = body.size_hint();

    if size_hint.exact() == Some(0) {
        store_empty_streaming_response(
            &context,
            EmptyStreamingCachePlan {
                final_key,
                vary,
                hash,
                paths,
                now,
                replaced_entry,
                status: parts.status,
                headers: parts.headers.clone(),
                freshness,
            },
        )
        .await;
        return passthrough_response(parts, body);
    }

    if let Err(error) = fs::create_dir_all(&paths.dir).await {
        tracing::warn!(
            zone = %context.zone.config.name,
            key_hash = %hash,
            %error,
            "failed to prepare cache directory for streaming store"
        );
        context.zone.record_write_error();
        return passthrough_response(parts, body);
    }

    let body_tmp = cache_entry_temp_body_path(&paths);
    let body_file = match File::create(&body_tmp).await {
        Ok(file) => file,
        Err(error) => {
            tracing::warn!(
                zone = %context.zone.config.name,
                key_hash = %hash,
                %error,
                "failed to create temporary cache body file"
            );
            context.zone.record_write_error();
            return passthrough_response(parts, body);
        }
    };

    let cache = ActiveStreamingCache {
        file: body_file,
        pending_write: None,
        finalize_after_pending_frame: false,
        plan: StreamingCachePlan {
            zone: context.zone,
            base_key: context.base_key,
            final_key,
            vary,
            status: parts.status,
            headers: parts.headers.clone(),
            freshness,
            now,
            hash,
            paths,
            body_tmp,
            body_size_bytes: 0,
            revalidating: context.revalidating,
            replaced_entry,
            _fill_guard: context._fill_guard,
        },
    };
    let body = boxed_body(StreamingCacheBody::new(body, size_hint, cache));
    http::Response::from_parts(parts, body)
}

async fn store_empty_streaming_response(
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

async fn update_index_after_store(
    zone: &Arc<CacheZoneRuntime>,
    key: String,
    entry: CacheIndexEntry,
    replaced_entry: Option<(String, CacheIndexEntry)>,
) -> std::collections::BTreeSet<String> {
    maintenance::update_index_after_store(zone, key, entry, replaced_entry).await
}

pub(super) fn duration_to_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn finalize_downstream_response(
    context: &CacheStoreContext,
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
) -> std::result::Result<HttpResponse, CacheStoreError> {
    if super::request::cacheable_range_request(&context.request, &context.policy)
        .is_some_and(|range| range.needs_downstream_trimming())
        && !downstream_range_trim_compatible(context, status, headers)
    {
        return build_response(status, headers, body);
    }

    finalize_response_for_request(status, headers, body, &context.request, &context.policy)
        .map_err(|error| CacheStoreError { source: Box::new(error) })
}

fn downstream_range_trim_compatible(
    context: &CacheStoreContext,
    status: StatusCode,
    headers: &HeaderMap,
) -> bool {
    status == StatusCode::PARTIAL_CONTENT
        && super::request::response_content_range_matches_request(
            &context.request,
            &context.policy,
            headers,
        )
}

fn build_response(
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
) -> std::result::Result<HttpResponse, CacheStoreError> {
    let mut response = http::Response::builder().status(status);
    *response.headers_mut().expect("response builder should expose headers") = headers.clone();
    response.body(full_body(Bytes::copy_from_slice(body))).map_err(|error| CacheStoreError {
        source: Box::new(std::io::Error::other(error.to_string())),
    })
}

fn passthrough_response(parts: http::response::Parts, body: HttpBody) -> HttpResponse {
    http::Response::from_parts(parts, body)
}

fn record_streaming_cache_write_error(plan: &StreamingCachePlan, error: &std::io::Error) {
    tracing::warn!(
        zone = %plan.zone.config.name,
        key_hash = %plan.hash,
        %error,
        "failed to stream cache entry body"
    );
    plan.zone.record_write_error();
}

fn start_streaming_cache_finalize(cache: ActiveStreamingCache) -> StreamingCacheFinalize {
    Box::pin(complete_streaming_cache(cache))
}

fn abandon_streaming_cache(cache: ActiveStreamingCache) {
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
