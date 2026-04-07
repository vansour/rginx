use super::access_log::{AccessLogContext, OwnedAccessLogContext, log_access_event};
use super::grpc::{
    GrpcRequestMetadata, GrpcStatusCode, grpc_error_response, grpc_observability,
    grpc_request_metadata, wrap_grpc_observability_response,
};
use super::response::{forbidden_response, full_body, text_response, too_many_requests_response};
use super::*;

#[derive(Clone, Copy)]
struct ListenerRequestContext<'a> {
    listener_id: &'a str,
    listener_tls_enabled: bool,
    request_body_read_timeout: Option<std::time::Duration>,
    max_request_body_bytes: Option<usize>,
}

pub async fn handle(
    request: Request<Incoming>,
    state: SharedState,
    remote_addr: SocketAddr,
    listener_id: &str,
) -> HttpResponse {
    let mut request = request;
    state.record_downstream_request();
    let active = state.snapshot().await;
    let config = active.config.clone();
    let listener = config
        .listener(listener_id)
        .cloned()
        .expect("listener id should remain available while serving requests");
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
    let path = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| request.uri().path().to_string());
    let request_path = request.uri().path().to_string();
    let grpc_request = grpc_request_metadata(&request_headers, &request_path);
    let route_match_context = route_match_context(&request_path, grpc_request);
    let started = Instant::now();
    let client_address = resolve_client_address(request.headers(), &listener.server, remote_addr);
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
    let listener_context = ListenerRequestContext {
        listener_id,
        listener_tls_enabled: listener.tls_enabled(),
        request_body_read_timeout: listener.server.request_body_read_timeout,
        max_request_body_bytes: listener.server.max_request_body_bytes,
    };
    let mut response = match selected_route {
        Some(route) => {
            if let Some(response) = authorize_route(&request_headers, &route, &client_address) {
                response
            } else if let Some(response) =
                enforce_rate_limit(&request_headers, &state, &route, &route_id, &client_address)
            {
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
    response = crate::compression::maybe_encode_response(&method, &request_headers, response).await;
    if method == Method::HEAD {
        response = strip_response_body(response);
    }
    response.headers_mut().insert("x-request-id", request_id_header);

    let status = response.status();
    state.record_downstream_response(status);
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let body_bytes_sent = response_body_bytes_sent(method.as_str(), &response);

    if let Some(grpc) = grpc_observability(grpc_request, response.headers()) {
        let format = config.server.access_log_format.clone();
        let context = OwnedAccessLogContext {
            request_id,
            method: method.as_str().to_string(),
            host,
            path,
            request_version,
            user_agent,
            referer,
            client_address,
            vhost: selected_vhost_id,
            route: route_id,
            status: status.as_u16(),
            elapsed_ms,
            downstream_scheme: downstream_scheme.to_string(),
            body_bytes_sent,
        };
        return wrap_grpc_observability_response(response, format, context, grpc);
    }

    log_access_event(
        config.server.access_log_format.as_ref(),
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
            body_bytes_sent,
            grpc: None,
        },
    );

    response
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
