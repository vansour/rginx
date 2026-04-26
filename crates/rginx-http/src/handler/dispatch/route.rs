use http::Request;
use http::header::HeaderValue;
use rginx_core::RouteAction;

use crate::state::{ActiveState, SharedState};

use super::super::grpc::{GrpcStatusCode, grpc_error_response};
use super::super::response::{full_body, too_early_response};
use super::{HttpBody, HttpResponse, ListenerRequestContext, RouteExecutionContext};

pub(super) fn early_data_rejection_response(request_headers: &http::HeaderMap) -> HttpResponse {
    grpc_error_response(
        request_headers,
        GrpcStatusCode::Unavailable,
        "early data rejected for non-replay-safe route",
    )
    .unwrap_or_else(too_early_response)
}

pub(super) async fn build_route_response(
    request: Request<HttpBody>,
    state: SharedState,
    route: RouteExecutionContext,
    active: ActiveState,
    listener: ListenerRequestContext<'_>,
    client_address: crate::client_ip::ClientAddress,
    request_id: &str,
) -> HttpResponse {
    match &route.action {
        RouteAction::Proxy(proxy) => {
            let downstream_proto = if listener.listener_tls_enabled { "https" } else { "http" };
            crate::proxy::forward_request(
                state,
                active.clients,
                request,
                listener.listener_id,
                proxy,
                client_address,
                crate::proxy::DownstreamRequestContext {
                    listener_id: listener.listener_id,
                    downstream_proto,
                    request_id,
                    options: crate::proxy::DownstreamRequestOptions {
                        request_body_read_timeout: listener.request_body_read_timeout,
                        max_request_body_bytes: listener.max_request_body_bytes,
                        request_buffering: route.request_buffering,
                        streaming_response_idle_timeout: route.streaming_response_idle_timeout,
                    },
                },
            )
            .await
        }
        RouteAction::Return(action) => {
            let body =
                action.body.clone().unwrap_or_else(|| match action.status.canonical_reason() {
                    Some(reason) => format!("{reason}\n"),
                    None if action.status.is_redirection() => String::from("Redirect\n"),
                    None => format!("{}\n", action.status.as_u16()),
                });
            let content_length = body.len();

            let mut builder = http::Response::builder()
                .status(action.status)
                .header("content-type", "text/plain; charset=utf-8")
                .header("content-length", content_length.to_string());

            if action.status.is_redirection()
                && !action.location.is_empty()
                && let Ok(location) = HeaderValue::from_str(&action.location)
            {
                builder = builder.header("location", location);
            }

            builder.body(full_body(body)).expect("return response builder should not fail")
        }
    }
}
