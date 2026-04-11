use super::access_log::{AccessLogContext, OwnedAccessLogContext, log_access_event};
use super::grpc::{
    GrpcRequestMetadata, GrpcStatsContext, GrpcStatusCode, grpc_error_response, grpc_observability,
    grpc_request_metadata, wrap_grpc_observability_response,
};
use super::response::{forbidden_response, full_body, text_response, too_many_requests_response};
use super::*;
use crate::client_ip::{ConnectionPeerAddrs, TlsClientIdentity};
use std::sync::Arc;

#[derive(Clone, Copy)]
struct ListenerRequestContext<'a> {
    listener_id: &'a str,
    listener_tls_enabled: bool,
    request_body_read_timeout: Option<std::time::Duration>,
    max_request_body_bytes: Option<usize>,
}

pub(super) struct FinalizedDownstreamResponse {
    pub(super) response: HttpResponse,
    pub(super) status: StatusCode,
    pub(super) body_bytes_sent: Option<u64>,
    pub(super) grpc: Option<super::grpc::GrpcObservability>,
}

pub async fn handle(
    request: Request<Incoming>,
    state: SharedState,
    connection: Arc<ConnectionPeerAddrs>,
    listener_id: &str,
) -> HttpResponse {
    let mut request = request;
    super::attach_connection_metadata(&mut request, connection.as_ref());
    let active = state.snapshot().await;
    let config = active.config.clone();
    let listener = config
        .listener(listener_id)
        .cloned()
        .expect("listener id should remain available while serving requests");
    let access_log_format = listener.server.access_log_format.clone();
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
    let response = match selected_route {
        Some(route) => {
            if let Some(response) = authorize_route(&request_headers, &route, &client_address) {
                state.record_route_access_denied(&route.id);
                response
            } else if let Some(response) =
                enforce_rate_limit(&request_headers, &state, &route, &route_id, &client_address)
            {
                state.record_route_rate_limited(&route.id);
                response
            } else {
                build_route_response(
                    request,
                    state.clone(),
                    route.action,
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
        request_id_header,
        response,
        grpc_request,
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

pub(super) async fn finalize_downstream_response(
    method: &Method,
    request_headers: &HeaderMap,
    request_id_header: HeaderValue,
    mut response: HttpResponse,
    grpc_request: Option<GrpcRequestMetadata<'_>>,
) -> FinalizedDownstreamResponse {
    // The final response pipeline is intentionally explicit:
    // 1. Detect gRPC early from response headers.
    // 2. Apply generic transforms only to non-gRPC, non-HEAD responses.
    // 3. Strip HEAD bodies last so headers still describe the payload shape.
    // 4. Add request-id after transforms so every returned response carries it.
    let grpc = grpc_observability(grpc_request, response.headers());
    if grpc.is_none() && *method != Method::HEAD {
        response =
            crate::compression::maybe_encode_response(method, request_headers, response).await;
    }
    if *method == Method::HEAD {
        response = strip_response_body(response);
    }
    response.headers_mut().insert("x-request-id", request_id_header);

    let status = response.status();
    let body_bytes_sent = response_body_bytes_sent(method.as_str(), &response);
    FinalizedDownstreamResponse { response, status, body_bytes_sent, grpc }
}

pub(super) fn response_body_bytes_sent(method: &str, response: &HttpResponse) -> Option<u64> {
    if method == Method::HEAD.as_str() {
        return Some(0);
    }

    response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

pub(super) fn http_version_label(version: Version) -> &'static str {
    match version {
        Version::HTTP_09 => "HTTP/0.9",
        Version::HTTP_10 => "HTTP/1.0",
        Version::HTTP_11 => "HTTP/1.1",
        Version::HTTP_2 => "HTTP/2.0",
        Version::HTTP_3 => "HTTP/3.0",
        _ => "HTTP/?",
    }
}

pub(super) fn header_value(headers: &HeaderMap, name: http::header::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|value| value.to_str().ok()).map(str::trim)
}

pub(super) fn request_host<'a>(headers: &'a HeaderMap, uri: &'a Uri) -> &'a str {
    headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| uri.authority().map(|authority| authority.as_str()))
        .unwrap_or_default()
}

fn select_vhost_for_request<'a>(
    config: &'a ConfigSnapshot,
    headers: &HeaderMap,
    uri: &Uri,
) -> &'a VirtualHost {
    let host = request_host(headers, uri).to_string();
    router::select_vhost(&config.vhosts, &config.default_vhost, &host)
}

fn route_match_context<'a>(
    request_path: &'a str,
    grpc_request: Option<GrpcRequestMetadata<'a>>,
) -> router::RouteMatchContext<'a> {
    let grpc = grpc_request.map(|metadata| router::GrpcRequestMatch {
        service: metadata.service,
        method: metadata.method,
    });

    router::RouteMatchContext { path: request_path, grpc }
}

#[cfg(test)]
pub(super) fn select_route_for_request<'a>(
    config: &'a ConfigSnapshot,
    headers: &HeaderMap,
    uri: &Uri,
) -> Option<&'a Route> {
    let vhost = select_vhost_for_request(config, headers, uri);
    let context = route_match_context(uri.path(), grpc_request_metadata(headers, uri.path()));
    router::select_route_in_vhost_with_context(vhost, &context)
}

pub(super) fn authorize_route(
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

pub(super) fn enforce_rate_limit(
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

async fn build_route_response(
    request: Request<Incoming>,
    state: SharedState,
    action: RouteAction,
    active: ActiveState,
    listener: ListenerRequestContext<'_>,
    client_address: ClientAddress,
    request_id: &str,
) -> HttpResponse {
    match &action {
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
                    },
                },
            )
            .await
        }
        RouteAction::Return(action) => {
            let body = action.body.clone().unwrap_or_else(|| {
                format!("{}\n", action.status.canonical_reason().unwrap_or("Redirect"))
            });
            let content_length = body.len();

            let mut builder = Response::builder()
                .status(action.status)
                .header("content-type", "text/plain; charset=utf-8")
                .header("content-length", content_length.to_string());

            if !action.location.is_empty() {
                builder = builder.header("location", &action.location);
            }

            builder.body(full_body(body)).expect("return response builder should not fail")
        }
    }
}

fn strip_response_body(response: HttpResponse) -> HttpResponse {
    let (parts, _body) = response.into_parts();
    Response::from_parts(parts, full_body(Bytes::new()))
}
