use std::sync::Arc;
use std::time::Instant;

use http::header::{REFERER, USER_AGENT};
use http::{HeaderValue, Request, StatusCode};
use rginx_core::RouteAction;

use crate::client_ip::{ConnectionPeerAddrs, TlsClientIdentity, resolve_client_address};
use crate::compression::ResponseCompressionOptions;
use crate::router;
use crate::state::SharedState;

use super::access_log::{AccessLogContext, OwnedAccessLogContext, log_access_event};
use super::grpc::{
    GrpcStatsContext, GrpcStatusCode, grpc_error_response, grpc_request_metadata,
    wrap_grpc_observability_response,
};
use super::response::{full_body, text_response};
use super::*;

mod authorize;
mod date;
mod response;
mod route;
mod select;

pub(super) use authorize::{authorize_route, enforce_rate_limit};
#[cfg(test)]
pub(super) use response::response_body_bytes_sent;
pub(super) use response::{finalize_downstream_response, header_value, http_version_label};
pub(super) use select::request_host;
#[cfg(test)]
pub(super) use select::select_route_for_request;

use response::alt_svc_header_value;
use route::{build_route_response, early_data_rejection_response};
use select::{route_match_context, select_vhost_for_request};

#[derive(Clone, Copy)]
struct ListenerRequestContext<'a> {
    listener_id: &'a str,
    listener_tls_enabled: bool,
    request_body_read_timeout: Option<std::time::Duration>,
    max_request_body_bytes: Option<usize>,
}

#[derive(Clone)]
struct RouteExecutionContext {
    action: RouteAction,
    request_buffering: rginx_core::RouteBufferingPolicy,
    streaming_response_idle_timeout: Option<std::time::Duration>,
    cache: Option<rginx_core::RouteCachePolicy>,
}

pub async fn handle(
    request: Request<HttpBody>,
    state: SharedState,
    connection: Arc<ConnectionPeerAddrs>,
    listener_id: &str,
) -> HttpResponse {
    let mut request = request;
    super::attach_connection_metadata(&mut request, connection.as_ref());
    let active = state.snapshot().await;
    let config = active.config.clone();
    let listener = if let Some(listener) = config.listener(listener_id).cloned() {
        listener
    } else {
        state
            .current_listener(listener_id)
            .await
            .expect("listener id should remain available while serving requests")
    };
    let access_log_format = listener.server.access_log_format.clone();
    let server_header = listener.server.server_header.clone();
    let method = request.method().clone();
    let request_version = request.version();
    let request_headers = request.headers().clone();
    let host = request_host(request.headers(), request.uri()).to_string();
    let user_agent = header_value(request.headers(), USER_AGENT).map(str::to_string);
    let referer = header_value(request.headers(), REFERER).map(str::to_string);
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| state.next_request_id());
    let request_id_header =
        HeaderValue::from_str(&request_id).expect("generated request ids should be valid headers");
    request.headers_mut().insert("x-request-id", request_id_header.clone());
    let tls_client_identity = request.extensions().get::<TlsClientIdentity>().cloned();
    let early_data =
        request.extensions().get::<EarlyDataFlag>().map(|flag| flag.0).unwrap_or(false);
    let path = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| request.uri().path().to_string());
    let request_path = request.uri().path().to_string();
    let grpc_request = grpc_request_metadata(&request_headers, &request_path);
    let route_match_context = route_match_context(&request_path, grpc_request);
    let started = Instant::now();
    let tls_version = connection.tls_version.clone();
    let tls_alpn = connection.tls_alpn.clone();
    let alt_svc_header = alt_svc_header_value(&listener, request_version);
    let client_address =
        resolve_client_address(request.headers(), &listener.server, connection.as_ref());
    let downstream_scheme = if listener.tls_enabled() { "https" } else { "http" };
    let (selected_vhost_id, selected_route) = {
        let selected_vhost =
            select_vhost_for_request(config.as_ref(), request.headers(), request.uri());
        (
            selected_vhost.id.clone(),
            router::select_route_in_vhost_with_context(selected_vhost, &route_match_context)
                .cloned(),
        )
    };
    let route_id = selected_route
        .as_ref()
        .map(|route| route.id.clone())
        .unwrap_or_else(|| "__unmatched__".to_string());
    let selected_route_id = selected_route.as_ref().map(|route| route.id.clone());
    let response_compression_options =
        selected_route.as_ref().map(ResponseCompressionOptions::for_route).unwrap_or_default();
    state.record_downstream_request(listener_id, &selected_vhost_id, selected_route_id.as_deref());
    if listener.server.tls.as_ref().and_then(|tls| tls.client_auth.as_ref()).is_some() {
        state.record_mtls_request(listener_id, tls_client_identity.is_some());
    }
    if let Some(grpc_request) = grpc_request {
        state.record_grpc_request(
            listener_id,
            &selected_vhost_id,
            selected_route_id.as_deref(),
            grpc_request.protocol,
        );
    }
    let listener_context = ListenerRequestContext {
        listener_id,
        listener_tls_enabled: listener.tls_enabled(),
        request_body_read_timeout: listener.server.request_body_read_timeout,
        max_request_body_bytes: listener.server.max_request_body_bytes,
    };
    let response = match selected_route.as_ref() {
        Some(route) => {
            if early_data && !route.allow_early_data {
                state.record_http3_early_data_rejected_request(listener_id);
                early_data_rejection_response(&request_headers)
            } else if let Some(response) = authorize_route(&request_headers, route, &client_address)
            {
                state.record_route_access_denied(&route.id);
                response
            } else if let Some(response) =
                enforce_rate_limit(&request_headers, &state, route, &route_id, &client_address)
            {
                state.record_route_rate_limited(&route.id);
                response
            } else {
                if early_data {
                    state.record_http3_early_data_accepted_request(listener_id);
                }
                build_route_response(
                    request,
                    state.clone(),
                    RouteExecutionContext {
                        action: route.action.clone(),
                        request_buffering: route.request_buffering,
                        streaming_response_idle_timeout: route.streaming_response_idle_timeout,
                        cache: route.cache.clone(),
                    },
                    active,
                    listener_context,
                    client_address.clone(),
                    &request_id,
                )
                .await
            }
        }
        None => {
            grpc_error_response(&request_headers, GrpcStatusCode::Unimplemented, "route not found")
                .unwrap_or_else(|| {
                    text_response(
                        StatusCode::NOT_FOUND,
                        "text/plain; charset=utf-8",
                        "route not found\n",
                    )
                })
        }
    };
    let finalized = finalize_downstream_response(
        &method,
        &request_headers,
        &response_compression_options,
        request_id_header,
        response,
        grpc_request,
        alt_svc_header,
        server_header,
    )
    .await;
    let status = finalized.status;
    state.record_downstream_response(
        listener_id,
        &selected_vhost_id,
        selected_route_id.as_deref(),
        status,
    );
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let body_bytes_sent = finalized.body_bytes_sent;
    let response = finalized.response;

    if let Some(grpc) = finalized.grpc {
        let context = OwnedAccessLogContext {
            request_id,
            method: method.as_str().to_string(),
            host,
            path,
            request_version,
            user_agent,
            referer,
            client_address,
            vhost: selected_vhost_id.clone(),
            route: route_id.clone(),
            status: status.as_u16(),
            elapsed_ms,
            downstream_scheme: downstream_scheme.to_string(),
            tls_version: tls_version.clone(),
            tls_alpn: tls_alpn.clone(),
            body_bytes_sent,
            tls_client_identity: tls_client_identity.clone(),
        };
        return wrap_grpc_observability_response(
            response,
            access_log_format,
            context,
            grpc,
            Some(GrpcStatsContext {
                state: state.clone(),
                listener_id: listener_id.to_string(),
                vhost_id: selected_vhost_id.clone(),
                route_id: selected_route_id,
            }),
        );
    }

    log_access_event(
        access_log_format.as_ref(),
        AccessLogContext {
            request_id: &request_id,
            method: method.as_str(),
            host: &host,
            path: &path,
            request_version,
            user_agent: user_agent.as_deref(),
            referer: referer.as_deref(),
            client_address: &client_address,
            vhost: &selected_vhost_id,
            route: &route_id,
            status: status.as_u16(),
            elapsed_ms,
            downstream_scheme,
            tls_version: tls_version.as_deref(),
            tls_alpn: tls_alpn.as_deref(),
            body_bytes_sent,
            tls_client_identity: tls_client_identity.as_ref(),
            grpc: None,
        },
    );

    response
}
