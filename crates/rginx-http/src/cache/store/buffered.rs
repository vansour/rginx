use bytes::Bytes;
use http_body_util::BodyExt;

use super::*;
use crate::handler::full_body;

pub(super) async fn store_buffered_response(
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

fn finalize_downstream_response(
    context: &CacheStoreContext,
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
) -> std::result::Result<HttpResponse, CacheStoreError> {
    if super::super::request::cacheable_range_request(&context.request, &context.policy)
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
        && super::super::request::response_content_range_matches_request(
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

pub(super) fn passthrough_response(parts: http::response::Parts, body: HttpBody) -> HttpResponse {
    http::Response::from_parts(parts, body)
}
