use super::*;

pub(super) async fn config_response(
    request: Request<Incoming>,
    state: SharedState,
    active: ActiveState,
) -> HttpResponse {
    if let Some(response) = authorize_config_request(request.headers(), &active.config.server) {
        return response;
    }

    match *request.method() {
        Method::GET | Method::HEAD => config_state_response(&state, &active).await,
        Method::PUT => config_update_response(request, state).await,
        _ => method_not_allowed_response(CONFIG_API_ALLOW),
    }
}

fn authorize_config_request(headers: &HeaderMap, server: &rginx_core::Server) -> Option<HttpResponse> {
    let expected_token = server.config_api_token.as_deref()?;
    let provided = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_bearer_token);

    if provided == Some(expected_token) {
        return None;
    }

    let mut response = json_error_response(StatusCode::UNAUTHORIZED, "config API authorization required");
    response.headers_mut().insert(
        http::header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer realm=\"rginx-config\""),
    );
    Some(response)
}

fn parse_bearer_token(value: &str) -> Option<&str> {
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }

    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

async fn config_state_response(state: &SharedState, active: &ActiveState) -> HttpResponse {
    let Some(config_path) = state.persistent_config_path() else {
        return json_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "dynamic config API is unavailable without a runtime-backed config path",
        );
    };
    let Some(config_source) = state.active_config_source().await else {
        return json_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "active configuration source is unavailable",
        );
    };

    json_response(
        StatusCode::OK,
        &ConfigPayload::from_active(active, &config_path, Some(config_source)),
    )
}

async fn config_update_response(request: Request<Incoming>, state: SharedState) -> HttpResponse {
    let collected = match request.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => {
            tracing::warn!(%error, "failed to read dynamic config request body");
            return json_error_response(
                StatusCode::BAD_REQUEST,
                "failed to read dynamic config request body",
            );
        }
    };

    if collected.is_empty() {
        return json_error_response(StatusCode::BAD_REQUEST, "config body must not be empty");
    }

    if collected.len() > MAX_CONFIG_API_BODY_BYTES {
        return json_error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            &format!("config body exceeds {MAX_CONFIG_API_BODY_BYTES} bytes"),
        );
    }

    let config_source = match String::from_utf8(collected.to_vec()) {
        Ok(config_source) => config_source,
        Err(_) => {
            return json_error_response(
                StatusCode::BAD_REQUEST,
                "config body must be valid UTF-8 RON",
            );
        }
    };

    match state.apply_config_source(config_source).await {
        Ok(_) => {
            let active = state.snapshot().await;
            let Some(config_path) = state.persistent_config_path() else {
                return json_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "dynamic config API is unavailable without a runtime-backed config path",
                );
            };

            tracing::info!(
                revision = active.revision,
                listen = %active.config.server.listen_addr,
                vhosts = active.config.total_vhost_count(),
                routes = active.config.total_route_count(),
                upstreams = active.config.upstreams.len(),
                "dynamic configuration updated"
            );

            json_response(StatusCode::OK, &ConfigPayload::from_active(&active, &config_path, None))
        }
        Err(Error::Config(message)) => json_error_response(StatusCode::BAD_REQUEST, &message),
        Err(error) => {
            tracing::warn!(%error, "failed to apply dynamic config update");
            json_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to apply dynamic config update",
            )
        }
    }
}

pub(super) fn status_response(active: &ActiveState) -> HttpResponse {
    let mut upstreams = active
        .config
        .upstreams
        .values()
        .map(|upstream| UpstreamStatusPayload {
            name: upstream.name.clone(),
            protocol: upstream.protocol.as_str(),
            load_balance: upstream.load_balance.as_str(),
            request_timeout_ms: upstream.request_timeout.as_millis() as u64,
            read_timeout_ms: upstream.request_timeout.as_millis() as u64,
            connect_timeout_ms: upstream.connect_timeout.as_millis() as u64,
            write_timeout_ms: upstream.write_timeout.as_millis() as u64,
            idle_timeout_ms: upstream.idle_timeout.as_millis() as u64,
            pool_idle_timeout_ms: upstream
                .pool_idle_timeout
                .map(|timeout| timeout.as_millis() as u64),
            pool_max_idle_per_host: upstream.pool_max_idle_per_host,
            tcp_keepalive_ms: upstream.tcp_keepalive.map(|timeout| timeout.as_millis() as u64),
            tcp_nodelay: upstream.tcp_nodelay,
            http2_keep_alive_interval_ms: upstream
                .http2_keep_alive_interval
                .map(|timeout| timeout.as_millis() as u64),
            http2_keep_alive_timeout_ms: upstream.http2_keep_alive_timeout.as_millis() as u64,
            http2_keep_alive_while_idle: upstream.http2_keep_alive_while_idle,
            active_health_check: upstream.active_health_check.as_ref().map(health_check_payload),
            peers: active.clients.peer_statuses(upstream.as_ref()),
        })
        .collect::<Vec<_>>();
    upstreams.sort_by(|left, right| left.name.cmp(&right.name));

    let payload = StatusPayload {
        revision: active.revision,
        listen: active.config.server.listen_addr.to_string(),
        vhost_count: active.config.total_vhost_count(),
        route_count: active.config.total_route_count(),
        upstream_count: active.config.upstreams.len(),
        upstreams,
    };

    json_response(StatusCode::OK, &payload)
}

pub(super) fn metrics_response(metrics: &Metrics) -> HttpResponse {
    let body = metrics.render_prometheus();
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/plain; version=0.0.4; charset=utf-8")
        .header("content-length", body.len().to_string())
        .body(full_body(body))
        .expect("response builder should not fail for metrics responses")
}

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
        .expect("response builder should not fail for static responses")
}

fn json_response(status: StatusCode, payload: &impl Serialize) -> HttpResponse {
    let body = serde_json::to_vec(payload).expect("status payload should serialize");
    Response::builder()
        .status(status)
        .header("content-type", "application/json; charset=utf-8")
        .header("content-length", body.len().to_string())
        .body(full_body(body))
        .expect("response builder should not fail for JSON responses")
}

fn json_error_response(status: StatusCode, error: &str) -> HttpResponse {
    json_response(status, &ErrorPayload { error: error.to_string() })
}

fn method_not_allowed_response(allow: &'static str) -> HttpResponse {
    let mut response = json_error_response(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
    response.headers_mut().insert(http::header::ALLOW, HeaderValue::from_static(allow));
    response
}

pub(crate) fn full_body(body: impl Into<Bytes>) -> HttpBody {
    Full::new(body.into())
        .map_err(|never: Infallible| -> BoxError { match never {} })
        .boxed_unsync()
}

fn health_check_payload(health_check: &ActiveHealthCheck) -> ActiveHealthCheckPayload {
    ActiveHealthCheckPayload {
        path: health_check.path.clone(),
        grpc_service: health_check.grpc_service.clone(),
        interval_ms: health_check.interval.as_millis() as u64,
        timeout_ms: health_check.timeout.as_millis() as u64,
        healthy_successes_required: health_check.healthy_successes_required,
    }
}

#[derive(Debug, Serialize)]
struct StatusPayload {
    revision: u64,
    listen: String,
    vhost_count: usize,
    route_count: usize,
    upstream_count: usize,
    upstreams: Vec<UpstreamStatusPayload>,
}

#[derive(Debug, Serialize)]
struct ConfigPayload {
    revision: u64,
    config_path: String,
    listen: String,
    vhost_count: usize,
    route_count: usize,
    upstream_count: usize,
    config: Option<String>,
}

impl ConfigPayload {
    fn from_active(active: &ActiveState, config_path: &Path, config: Option<String>) -> Self {
        Self {
            revision: active.revision,
            config_path: config_path.display().to_string(),
            listen: active.config.server.listen_addr.to_string(),
            vhost_count: active.config.total_vhost_count(),
            route_count: active.config.total_route_count(),
            upstream_count: active.config.upstreams.len(),
            config,
        }
    }
}

#[derive(Debug, Serialize)]
struct UpstreamStatusPayload {
    name: String,
    protocol: &'static str,
    load_balance: &'static str,
    request_timeout_ms: u64,
    read_timeout_ms: u64,
    connect_timeout_ms: u64,
    write_timeout_ms: u64,
    idle_timeout_ms: u64,
    pool_idle_timeout_ms: Option<u64>,
    pool_max_idle_per_host: usize,
    tcp_keepalive_ms: Option<u64>,
    tcp_nodelay: bool,
    http2_keep_alive_interval_ms: Option<u64>,
    http2_keep_alive_timeout_ms: u64,
    http2_keep_alive_while_idle: bool,
    active_health_check: Option<ActiveHealthCheckPayload>,
    peers: Vec<PeerStatusSnapshot>,
}

#[derive(Debug, Serialize)]
struct ActiveHealthCheckPayload {
    path: String,
    grpc_service: Option<String>,
    interval_ms: u64,
    timeout_ms: u64,
    healthy_successes_required: u32,
}

#[derive(Debug, Serialize)]
struct ErrorPayload {
    error: String,
}
