use std::error::Error as StdError;
use std::future::Future;

use super::*;

pub(super) fn invalid_downstream_request_body_error(error: &(dyn StdError + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(candidate) = current {
        if let Some(io_error) = candidate.downcast_ref::<std::io::Error>()
            && matches!(
                io_error.kind(),
                std::io::ErrorKind::InvalidData | std::io::ErrorKind::InvalidInput
            )
        {
            return true;
        }

        let message = candidate.to_string();
        if message.contains("grpc-web") && message.contains("invalid") {
            return true;
        }

        current = candidate.source();
    }

    false
}

pub(super) fn downstream_request_body_too_large_error(error: &(dyn StdError + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(candidate) = current {
        if candidate.to_string().contains("request body exceeded configured limit of") {
            return true;
        }

        current = candidate.source();
    }

    false
}

pub(crate) async fn wait_for_upstream_stage<T>(
    request_timeout: Duration,
    upstream_name: &str,
    stage: &str,
    future: impl Future<Output = T>,
) -> Result<T, Error> {
    tokio::time::timeout(request_timeout, future).await.map_err(|_| {
        Error::Server(format!(
            "upstream `{upstream_name}` {stage} timed out after {} ms",
            request_timeout.as_millis()
        ))
    })
}

pub(super) fn gateway_timeout(request_headers: &HeaderMap, message: String) -> HttpResponse {
    grpc_error_response(request_headers, GrpcStatusCode::DeadlineExceeded, &message).unwrap_or_else(
        || {
            crate::handler::text_response(
                StatusCode::GATEWAY_TIMEOUT,
                "text/plain; charset=utf-8",
                message,
            )
        },
    )
}

pub(super) fn grpc_timeout_message(upstream_name: &str, timeout: Duration) -> String {
    format!("upstream `{upstream_name}` timed out after {} ms", timeout.as_millis())
}

pub(super) fn bad_gateway(request_headers: &HeaderMap, message: String) -> HttpResponse {
    grpc_error_response(request_headers, GrpcStatusCode::Unavailable, &message).unwrap_or_else(
        || {
            crate::handler::text_response(
                StatusCode::BAD_GATEWAY,
                "text/plain; charset=utf-8",
                message,
            )
        },
    )
}

pub(super) fn payload_too_large(request_headers: &HeaderMap, message: String) -> HttpResponse {
    grpc_error_response(request_headers, GrpcStatusCode::ResourceExhausted, &message)
        .unwrap_or_else(|| {
            crate::handler::text_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "text/plain; charset=utf-8",
                message,
            )
        })
}

pub(super) fn bad_request(request_headers: &HeaderMap, message: String) -> HttpResponse {
    grpc_error_response(request_headers, GrpcStatusCode::InvalidArgument, &message).unwrap_or_else(
        || {
            crate::handler::text_response(
                StatusCode::BAD_REQUEST,
                "text/plain; charset=utf-8",
                message,
            )
        },
    )
}

pub(super) fn unsupported_media_type(request_headers: &HeaderMap, message: String) -> HttpResponse {
    grpc_error_response(request_headers, GrpcStatusCode::InvalidArgument, &message).unwrap_or_else(
        || {
            crate::handler::text_response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "text/plain; charset=utf-8",
                message,
            )
        },
    )
}
