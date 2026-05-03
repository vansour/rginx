use std::path::PathBuf;
use std::time::Duration;

use bytes::Bytes;
use http::StatusCode;
use http::header::HeaderMap;
use hyper::body::Body as _;
use tokio::fs::{self, File};
use tokio::sync::mpsc;

mod body;
mod finalize;

use super::super::entry::DownstreamRangeTrimPlan;
use super::super::fill::{CacheFillReadState, inflight_fill_body};
use super::range::build_downstream_response;
use super::*;
use body::{StreamingCacheBody, spawn_streaming_origin_fill};
use finalize::spawn_streaming_cache_writer;

const STREAMING_CACHE_WRITE_QUEUE_DEPTH: usize = 8;

struct StreamingCachePlan {
    zone: Arc<CacheZoneRuntime>,
    base_key: String,
    final_key: String,
    vary: Vec<super::super::CachedVaryHeaderValue>,
    tags: Vec<String>,
    status: StatusCode,
    headers: HeaderMap,
    freshness: super::super::policy::ResponseFreshness,
    now: u64,
    hash: String,
    paths: super::super::entry::CachePaths,
    body_tmp: PathBuf,
    max_entry_bytes: usize,
    grace: Option<Duration>,
    keep: Option<Duration>,
    pass_ttl: Option<Duration>,
    expected_body_bytes: Option<usize>,
    revalidating: bool,
    replaced_entry: Option<(String, CacheIndexEntry)>,
    _fill_guard: Option<super::super::CacheFillGuard>,
    fill_state: Option<Arc<CacheFillReadState>>,
}

pub(super) enum StreamingCacheWriteMessage {
    Data(Bytes),
    Finish { trailers: Option<HeaderMap> },
    Abort,
}

pub(super) struct StreamingCacheWriter {
    tx: mpsc::Sender<StreamingCacheWriteMessage>,
    fill_state: Option<Arc<CacheFillReadState>>,
}

impl StreamingCacheWriter {
    fn new(
        tx: mpsc::Sender<StreamingCacheWriteMessage>,
        fill_state: Option<Arc<CacheFillReadState>>,
    ) -> Self {
        Self { tx, fill_state }
    }

    fn try_send_data(&self, bytes: Bytes) -> bool {
        self.tx.try_send(StreamingCacheWriteMessage::Data(bytes)).is_ok()
    }

    fn abort(&self, reason: &str) {
        if let Some(fill_state) = self.fill_state.as_ref() {
            fill_state.fail(reason);
        }
        let _ = self.tx.try_send(StreamingCacheWriteMessage::Abort);
    }

    fn try_finish(&self, trailers: Option<HeaderMap>) -> bool {
        let sent = self.tx.try_send(StreamingCacheWriteMessage::Finish { trailers }).is_ok();
        if sent && let Some(fill_state) = self.fill_state.as_ref() {
            fill_state.mark_upstream_complete();
        }
        sent
    }

    async fn send_data(&self, bytes: Bytes) -> bool {
        self.tx.send(StreamingCacheWriteMessage::Data(bytes)).await.is_ok()
    }

    async fn finish(self, trailers: Option<HeaderMap>) -> bool {
        let sent = self.tx.send(StreamingCacheWriteMessage::Finish { trailers }).await.is_ok();
        if sent && let Some(fill_state) = self.fill_state.as_ref() {
            fill_state.mark_upstream_complete();
        }
        sent
    }
}

pub(super) async fn store_streaming_response(
    context: CacheStoreContext,
    parts: http::response::Parts,
    body: HttpBody,
    storable: bool,
    no_cache: bool,
    downstream_range_trim: Option<DownstreamRangeTrimPlan>,
) -> HttpResponse {
    let now = unix_time_ms(SystemTime::now());
    if !storable || no_cache {
        if should_remember_hit_for_pass(&context, &parts.headers, no_cache) {
            for removed_hash in remember_hit_for_pass(&context, &parts.headers, now).await {
                remove_cache_files_if_unreferenced(context.zone.as_ref(), &removed_hash).await;
            }
        }
        return build_downstream_response(parts, body, downstream_range_trim);
    }

    let freshness = response_freshness(&context, parts.status, &parts.headers);
    if !freshness_is_cacheable(&freshness) {
        if should_remember_hit_for_pass(&context, &parts.headers, false) {
            for removed_hash in remember_hit_for_pass(&context, &parts.headers, now).await {
                remove_cache_files_if_unreferenced(context.zone.as_ref(), &removed_hash).await;
            }
        }
        return build_downstream_response(parts, body, downstream_range_trim);
    }

    let (final_key, vary, tags) =
        cache_final_key_for_response(&context, &context.request, &parts.headers);
    let admission =
        record_cache_admission_attempt(&context.zone, &final_key, context.policy.min_uses).await;
    if !admission.admitted {
        return build_downstream_response(parts, body, downstream_range_trim);
    }

    let upstream_status = parts.status;
    let upstream_headers = parts.headers.clone();
    let hash = context
        .cached_entry
        .as_ref()
        .filter(|_| context.key == final_key)
        .map(|entry| entry.hash.clone())
        .unwrap_or_else(|| cache_key_hash(&final_key));
    let paths = cache_paths_for_zone(context.zone.config.as_ref(), &hash);
    let max_entry_bytes = context.zone.config.max_entry_bytes;
    let replaced_entry = context
        .cached_entry
        .as_ref()
        .filter(|_| context.key != final_key)
        .map(|entry| (context.key.clone(), entry.clone()));
    let size_hint = body.size_hint();
    let expected_body_bytes = size_hint.exact().and_then(|exact| usize::try_from(exact).ok());
    if expected_body_bytes.is_some_and(|body_size_bytes| body_size_bytes > max_entry_bytes) {
        for removed_hash in remember_hit_for_pass(&context, &upstream_headers, now).await {
            remove_cache_files_if_unreferenced(context.zone.as_ref(), &removed_hash).await;
        }
        return build_downstream_response(parts, body, downstream_range_trim);
    }

    if let Err(error) = fs::create_dir_all(&paths.dir).await {
        tracing::warn!(
            zone = %context.zone.config.name,
            key_hash = %hash,
            %error,
            "failed to prepare cache directory for streaming store"
        );
        context.zone.record_write_error();
        return build_downstream_response(parts, body, downstream_range_trim);
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
            return build_downstream_response(parts, body, downstream_range_trim);
        }
    };
    let fill_state = context._fill_guard.as_ref().and_then(|fill_guard| {
        context.zone.attach_fill_read_state(
            &fill_guard.key,
            fill_guard.generation,
            Arc::new(CacheFillReadState::new(
                upstream_status,
                upstream_headers.clone(),
                body_tmp.clone(),
                paths.body.clone(),
                fill_guard.notify.clone(),
                fill_guard.external_state.clone(),
            )),
        )
    });
    let writer = spawn_streaming_cache_writer(
        StreamingCachePlan {
            zone: context.zone,
            base_key: context.base_key,
            final_key,
            vary,
            tags,
            status: upstream_status,
            headers: upstream_headers,
            freshness,
            now,
            hash,
            paths,
            body_tmp,
            max_entry_bytes,
            grace: context.policy.grace,
            keep: context.policy.keep,
            pass_ttl: context.policy.pass_ttl,
            expected_body_bytes,
            revalidating: context.revalidating,
            replaced_entry,
            _fill_guard: context._fill_guard,
            fill_state: fill_state.clone(),
        },
        body_file,
        STREAMING_CACHE_WRITE_QUEUE_DEPTH,
    );
    match (fill_state, downstream_range_trim) {
        (Some(fill_state_for_body), Some(trim_plan)) => {
            spawn_streaming_origin_fill(body, writer, Some(fill_state_for_body.clone()));
            build_downstream_response(
                parts,
                inflight_fill_body(fill_state_for_body),
                Some(trim_plan),
            )
        }
        (_, trim_plan) => build_downstream_response(
            parts,
            StreamingCacheBody::new(body, size_hint, writer, max_entry_bytes),
            trim_plan,
        ),
    }
}
