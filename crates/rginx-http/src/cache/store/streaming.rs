use std::path::PathBuf;

use http::StatusCode;
use http::header::HeaderMap;
use hyper::body::Body as _;
use tokio::fs::{self, File};

mod body;
mod finalize;

use super::buffered::passthrough_response;
use super::*;
use crate::handler::boxed_body;
use body::StreamingCacheBody;
use finalize::store_empty_streaming_response;

struct StreamingCachePlan {
    zone: Arc<CacheZoneRuntime>,
    base_key: String,
    final_key: String,
    vary: Vec<super::super::CachedVaryHeaderValue>,
    status: StatusCode,
    headers: HeaderMap,
    freshness: super::super::policy::ResponseFreshness,
    now: u64,
    hash: String,
    paths: super::super::entry::CachePaths,
    body_tmp: PathBuf,
    body_size_bytes: usize,
    revalidating: bool,
    replaced_entry: Option<(String, CacheIndexEntry)>,
    _fill_guard: Option<super::super::CacheFillGuard>,
}

pub(super) struct EmptyStreamingCachePlan {
    final_key: String,
    vary: Vec<super::super::CachedVaryHeaderValue>,
    hash: String,
    paths: super::super::entry::CachePaths,
    now: u64,
    replaced_entry: Option<(String, CacheIndexEntry)>,
    status: StatusCode,
    headers: HeaderMap,
    freshness: super::super::policy::ResponseFreshness,
}

pub(super) struct ActiveStreamingCache {
    file: File,
    pending_write: Option<PendingCacheWrite>,
    finalize_after_pending_frame: bool,
    plan: StreamingCachePlan,
}

struct PendingCacheWrite {
    bytes: bytes::Bytes,
    written: usize,
}

pub(super) type StreamingCacheFinalize =
    std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>;

impl PendingCacheWrite {
    fn new(bytes: bytes::Bytes) -> Self {
        Self { bytes, written: 0 }
    }

    fn remaining(&self) -> &[u8] {
        &self.bytes[self.written..]
    }
}

pub(super) async fn store_streaming_response(
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

    let body = boxed_body(StreamingCacheBody::new(
        body,
        size_hint,
        ActiveStreamingCache {
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
        },
    ));
    http::Response::from_parts(parts, body)
}
