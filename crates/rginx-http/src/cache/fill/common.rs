use std::time::Duration;

use bytes::Bytes;
use http::{HeaderMap, Response, StatusCode};
use hyper::body::SizeHint;

use super::super::entry::{DownstreamRangeTrimPlan, downstream_range_trim_plan};
use super::super::{CacheRequest, RouteCachePolicy};
use crate::handler::full_body;

pub(super) const IN_FLIGHT_FILL_READ_CHUNK_BYTES: usize = 16 * 1024;
pub(super) const EXTERNAL_FILL_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub(super) fn inflight_response_parts(
    status: StatusCode,
    headers: &HeaderMap,
    request: &CacheRequest,
    policy: &RouteCachePolicy,
) -> std::io::Result<(http::response::Parts, Option<DownstreamRangeTrimPlan>)> {
    let trim_plan = downstream_range_trim_plan(status, headers, request, policy)?;
    let mut response = Response::builder().status(status);
    *response.headers_mut().expect("response builder should expose headers") = headers.clone();
    let (parts, _) = response
        .body(full_body(Bytes::new()))
        .map_err(|error| std::io::Error::other(error.to_string()))?
        .into_parts();
    Ok((parts, trim_plan))
}

pub(super) fn size_hint_from_headers(headers: &HeaderMap) -> SizeHint {
    let mut hint = SizeHint::default();
    if let Some(content_length) = headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
    {
        hint.set_exact(content_length);
    }
    hint
}
