use super::*;

struct CachedContentRange {
    start: u64,
    end: u64,
    total: Option<u64>,
}

pub(in crate::cache) fn finalize_response_for_request(
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
    request: &CacheRequest,
    policy: &rginx_core::RouteCachePolicy,
) -> std::io::Result<HttpResponse> {
    let Some(request_range) = super::super::request::cacheable_range_request(request, policy)
        .filter(|range| range.needs_downstream_trimming())
    else {
        return build_response(status, headers, body.to_vec());
    };

    let mut headers = headers.clone();
    let cached_range = parse_cached_content_range(&headers)?;
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

    let body = if body.is_empty() {
        Vec::new()
    } else {
        let start_offset = usize::try_from(request_range.request_start - cached_range.start)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let end_offset = usize::try_from(response_end - cached_range.start + 1)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        body.get(start_offset..end_offset)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "requested range exceeds cached slice body bounds",
                )
            })?
            .to_vec()
    };

    let response_len = usize::try_from(response_end - request_range.request_start + 1)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    headers.insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&response_len.to_string())
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?,
    );
    headers.insert(
        CONTENT_RANGE,
        HeaderValue::from_str(&format!(
            "bytes {}-{}/{}",
            request_range.request_start,
            response_end,
            cached_range.total.map(|total| total.to_string()).unwrap_or_else(|| "*".to_string())
        ))
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?,
    );
    build_response(StatusCode::PARTIAL_CONTENT, &headers, body)
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

fn build_response(
    status: StatusCode,
    headers: &HeaderMap,
    body: Vec<u8>,
) -> std::io::Result<HttpResponse> {
    let mut response = Response::builder().status(status);
    *response.headers_mut().expect("response builder should expose headers") = headers.clone();
    response
        .body(full_body(Bytes::from(body)))
        .map_err(|error| std::io::Error::other(error.to_string()))
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
