use super::super::store::range::build_downstream_response;
use super::*;
use crate::handler::BoxError;
use bytes::Bytes;
use http::Response;
use hyper::body::Body;

mod body;

struct CachedContentRange {
    start: u64,
    end: u64,
    total: Option<u64>,
}

#[derive(Debug, Clone)]
pub(in crate::cache) struct DownstreamRangeTrimPlan {
    headers: HeaderMap,
    skip_bytes: usize,
    emit_bytes: usize,
}

pub(in crate::cache) use body::CachedFileBody;

pub(in crate::cache) fn finalize_response_for_request<B>(
    status: StatusCode,
    headers: &HeaderMap,
    body: B,
    request: &CacheRequest,
    policy: &rginx_core::RouteCachePolicy,
) -> std::io::Result<HttpResponse>
where
    B: Body<Data = Bytes> + Send + 'static,
    B::Error: Into<BoxError> + 'static,
{
    let trim_plan = downstream_range_trim_plan(status, headers, request, policy)?;
    let mut response = Response::builder().status(status);
    *response.headers_mut().expect("response builder should expose headers") = headers.clone();
    let (parts, _) = response
        .body(crate::handler::full_body(Bytes::new()))
        .map_err(|error| std::io::Error::other(error.to_string()))?
        .into_parts();
    Ok(build_downstream_response(parts, body, trim_plan))
}

pub(in crate::cache) fn downstream_range_trim_plan(
    status: StatusCode,
    headers: &HeaderMap,
    request: &CacheRequest,
    policy: &rginx_core::RouteCachePolicy,
) -> std::io::Result<Option<DownstreamRangeTrimPlan>> {
    let Some(request_range) = super::super::request::cacheable_range_request(request, policy)
        .filter(|range| range.needs_downstream_trimming())
    else {
        return Ok(None);
    };
    if status != StatusCode::PARTIAL_CONTENT
        || !super::super::request::response_content_range_matches_request(request, policy, headers)
    {
        return Ok(None);
    }

    let cached_range = parse_cached_content_range(headers)?;
    if cached_range.start != request_range.cache_start {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "cached content-range start `{}` does not match expected slice start `{}`",
                cached_range.start, request_range.cache_start
            ),
        ));
    }

    let response_end = request_range.request_end.min(cached_range.end);
    if request_range.request_start < cached_range.start
        || request_range.request_start > response_end
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "requested range `{}-{}` is not satisfiable from cached slice `{}-{}`",
                request_range.request_start,
                request_range.request_end,
                cached_range.start,
                cached_range.end
            ),
        ));
    }

    let skip_bytes = usize::try_from(request_range.request_start - cached_range.start)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let emit_bytes = usize::try_from(response_end - request_range.request_start + 1)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let mut trimmed_headers = headers.clone();
    update_downstream_range_headers(
        &mut trimmed_headers,
        request_range.request_start,
        response_end,
        cached_range.total,
        emit_bytes,
    )?;

    Ok(Some(DownstreamRangeTrimPlan { headers: trimmed_headers, skip_bytes, emit_bytes }))
}

pub(super) fn cached_headers(headers: &HeaderMap, body_size_bytes: usize) -> Vec<CachedHeader> {
    let mut headers = headers.clone();
    let had_content_length = headers.contains_key(CONTENT_LENGTH);
    remove_cache_hop_by_hop_headers(&mut headers);
    headers.remove(CACHE_STATUS_HEADER);
    headers.remove(CONTENT_LENGTH);
    if had_content_length || body_size_bytes > 0 {
        headers.insert(
            CONTENT_LENGTH,
            HeaderValue::from_str(&body_size_bytes.to_string())
                .expect("cache body length should fit in a header"),
        );
    }

    headers
        .iter()
        .map(|(name, value)| CachedHeader {
            name: name.as_str().to_string(),
            value: value.as_bytes().to_vec(),
        })
        .collect()
}

fn parse_cached_content_range(headers: &HeaderMap) -> std::io::Result<CachedContentRange> {
    let value = headers.get(CONTENT_RANGE).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "cached slice metadata is missing Content-Range",
        )
    })?;
    let value = value
        .to_str()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let value = value.trim().strip_prefix("bytes ").ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cached Content-Range `{value}` is not a byte range"),
        )
    })?;
    let (range, total) = value.split_once('/').ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cached Content-Range `{value}` is malformed"),
        )
    })?;
    let (start, end) = range.split_once('-').ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cached Content-Range `{value}` is malformed"),
        )
    })?;
    let start = start
        .trim()
        .parse::<u64>()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let end = end
        .trim()
        .parse::<u64>()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let total = match total.trim() {
        "*" => None,
        value => Some(
            value
                .parse::<u64>()
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?,
        ),
    };
    Ok(CachedContentRange { start, end, total })
}

fn update_downstream_range_headers(
    headers: &mut HeaderMap,
    response_start: u64,
    response_end: u64,
    total: Option<u64>,
    response_len: usize,
) -> std::io::Result<()> {
    headers.insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&response_len.to_string())
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?,
    );
    headers.insert(
        CONTENT_RANGE,
        HeaderValue::from_str(&format!(
            "bytes {}-{}/{}",
            response_start,
            response_end,
            total.map(|value| value.to_string()).unwrap_or_else(|| "*".to_string())
        ))
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?,
    );
    Ok(())
}

fn remove_cache_hop_by_hop_headers(headers: &mut HeaderMap) {
    let mut extra_headers = Vec::new();
    for value in headers.get_all(CONNECTION) {
        let Ok(value) = value.to_str() else {
            continue;
        };
        for token in value.split(',').map(str::trim).filter(|token| !token.is_empty()) {
            if let Ok(name) = HeaderName::from_bytes(token.as_bytes()) {
                extra_headers.push(name);
            }
        }
    }

    for name in extra_headers {
        headers.remove(name);
    }
    for name in [
        CONNECTION,
        PROXY_AUTHENTICATE,
        PROXY_AUTHORIZATION,
        TE,
        TRAILER,
        TRANSFER_ENCODING,
        UPGRADE,
    ] {
        headers.remove(name);
    }
    headers.remove("keep-alive");
    headers.remove("proxy-connection");
}

impl DownstreamRangeTrimPlan {
    pub(in crate::cache) fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    pub(in crate::cache) fn skip_bytes(&self) -> usize {
        self.skip_bytes
    }

    pub(in crate::cache) fn emit_bytes(&self) -> usize {
        self.emit_bytes
    }
}
