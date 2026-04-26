use http::HeaderMap;
use rginx_core::Route;

use crate::client_ip::ClientAddress;
use crate::state::SharedState;

use super::super::grpc::{GrpcStatusCode, grpc_error_response};
use super::super::response::{forbidden_response, too_many_requests_response};
use super::HttpResponse;

pub(in crate::handler) fn authorize_route(
    request_headers: &HeaderMap,
    route: &Route,
    client_address: &ClientAddress,
) -> Option<HttpResponse> {
    if route.access_control.allows(client_address.client_ip) {
        return None;
    }

    tracing::warn!(
        client_ip = %client_address.client_ip,
        peer_addr = %client_address.peer_addr,
        route = %route.id,
        "request denied by access control"
    );
    Some(
        grpc_error_response(request_headers, GrpcStatusCode::PermissionDenied, "forbidden")
            .unwrap_or_else(forbidden_response),
    )
}

pub(in crate::handler) fn enforce_rate_limit(
    request_headers: &HeaderMap,
    state: &SharedState,
    route: &Route,
    route_metric_label: &str,
    client_address: &ClientAddress,
) -> Option<HttpResponse> {
    if state.rate_limiters().check(
        route_metric_label,
        client_address.client_ip,
        route.rate_limit.as_ref(),
    ) {
        return None;
    }

    tracing::warn!(
        client_ip = %client_address.client_ip,
        peer_addr = %client_address.peer_addr,
        route = %route.id,
        "request rejected by route rate limit"
    );
    Some(
        grpc_error_response(
            request_headers,
            GrpcStatusCode::ResourceExhausted,
            "too many requests",
        )
        .unwrap_or_else(too_many_requests_response),
    )
}
