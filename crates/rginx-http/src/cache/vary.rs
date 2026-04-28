use http::header::{ACCEPT_ENCODING, HeaderMap, HeaderName};

use super::{CacheRequest, CachedVaryHeaderValue};

pub(super) fn canonical_vary_headers(vary: &[CachedVaryHeaderValue]) -> Vec<CachedVaryHeaderValue> {
    let mut canonical = vary.to_vec();
    canonical.sort_by(|left, right| {
        left.name
            .as_str()
            .cmp(right.name.as_str())
            .then_with(|| left.value.as_deref().cmp(&right.value.as_deref()))
    });
    canonical
}

pub(super) fn sorted_vary_dimension_names(vary: &[CachedVaryHeaderValue]) -> Vec<String> {
    let mut names = vary.iter().map(|header| header.name.as_str().to_string()).collect::<Vec<_>>();
    names.sort_unstable();
    names
}

pub(super) fn matches_vary_headers(request: &CacheRequest, vary: &[CachedVaryHeaderValue]) -> bool {
    vary.iter().all(|header| {
        normalized_request_header_values(&request.headers, &header.name) == header.value
    })
}

pub(super) fn normalized_request_header_values(
    headers: &HeaderMap,
    name: &HeaderName,
) -> Option<String> {
    if *name == ACCEPT_ENCODING {
        return normalized_accept_encoding(headers);
    }

    let mut values =
        headers.get_all(name).iter().filter_map(|header| header.to_str().ok()).peekable();
    values.peek()?;
    let mut rendered = String::new();
    while let Some(value) = values.next() {
        rendered.push_str(value.trim());
        if values.peek().is_some() {
            rendered.push(',');
        }
    }
    Some(rendered)
}

pub(super) fn normalized_accept_encoding(headers: &HeaderMap) -> Option<String> {
    let mut tokens = Vec::new();
    for value in headers.get_all(ACCEPT_ENCODING) {
        let value = value.to_str().ok()?;
        tokens.extend(value.split(',').map(str::trim).filter(|token| !token.is_empty()));
    }
    if tokens.is_empty() {
        return None;
    }
    Some(tokens.join(",").to_ascii_lowercase())
}
