use std::convert::Infallible;
use std::error::Error as StdError;

use bytes::Bytes;
use http::StatusCode;
use http_body_util::BodyExt;
use http_body_util::{Full, combinators::UnsyncBoxBody};
use hyper::Response;

pub(crate) type BoxError = Box<dyn StdError + Send + Sync>;
pub(crate) type HttpBody = UnsyncBoxBody<Bytes, BoxError>;
pub(crate) type HttpResponse = Response<HttpBody>;

pub(super) fn forbidden_response() -> HttpResponse {
    text_response(StatusCode::FORBIDDEN, "text/plain; charset=utf-8", "forbidden\n")
}

pub(super) fn too_many_requests_response() -> HttpResponse {
    text_response(
        StatusCode::TOO_MANY_REQUESTS,
        "text/plain; charset=utf-8",
        "hold your horses! too many requests\n",
    )
}

pub(crate) fn text_response(
    status: StatusCode,
    content_type: &str,
    body: impl Into<Bytes>,
) -> HttpResponse {
    let body = body.into();
    Response::builder()
        .status(status)
        .header("content-type", content_type)
        .header("content-length", body.len().to_string())
        .body(full_body(body))
        .expect("response builder should not fail for text responses")
}

pub(crate) fn full_body(body: impl Into<Bytes>) -> HttpBody {
    Full::new(body.into())
        .map_err(|never: Infallible| -> BoxError { match never {} })
        .boxed_unsync()
}
