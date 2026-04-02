use super::clients::ProxyClients;
use super::*;

mod grpc_health_codec;
mod registry;

#[allow(unused_imports)]
pub(crate) use grpc_health_codec::{
    GrpcHealthProbeResult, GrpcHealthServingStatus, decode_grpc_health_check_response,
    encode_grpc_health_check_request, evaluate_grpc_health_probe_response,
};
pub(crate) use registry::{
    ActivePeerBody, ActivePeerGuard, ActiveProbeStatus, PeerFailureStatus, PeerHealthRegistry,
    PeerStatusSnapshot, SelectedPeers,
};

pub async fn probe_upstream_peer(
    clients: ProxyClients,
    metrics: Metrics,
    upstream: Arc<Upstream>,
    peer: UpstreamPeer,
) {
    let Some(check) = upstream.active_health_check.as_ref() else {
        return;
    };

    let client = match clients.for_upstream(upstream.as_ref()) {
        Ok(client) => client,
        Err(error) => {
            let transitioned = clients.record_active_peer_failure(&upstream.name, &peer.url);
            metrics.record_active_health_check(&upstream.name, &peer.url, "client_unavailable");
            record_active_transition(
                &metrics,
                &upstream.name,
                &peer.url,
                transitioned,
                "unhealthy",
                "client_unavailable",
            );
            let level = if transitioned { "unhealthy" } else { "still unhealthy" };
            tracing::warn!(
                upstream = %upstream.name,
                peer = %peer.url,
                %error,
                state = level,
                "active health check could not acquire a proxy client"
            );
            return;
        }
    };

    let request = match build_active_health_request(upstream.as_ref(), &peer, check) {
        Ok(request) => request,
        Err(error) => {
            let transitioned = clients.record_active_peer_failure(&upstream.name, &peer.url);
            metrics.record_active_health_check(&upstream.name, &peer.url, "request_build_error");
            record_active_transition(
                &metrics,
                &upstream.name,
                &peer.url,
                transitioned,
                "unhealthy",
                "request_build_error",
            );
            let level = if transitioned { "unhealthy" } else { "still unhealthy" };
            tracing::warn!(
                upstream = %upstream.name,
                peer = %peer.url,
                path = %check.path,
                %error,
                state = level,
                "active health check request could not be built"
            );
            return;
        }
    };

    match tokio::time::timeout(check.timeout, client.request(request)).await {
        Ok(Ok(response)) if check.grpc_service.is_some() => {
            match tokio::time::timeout(check.timeout, evaluate_grpc_health_probe_response(response))
                .await
            {
                Ok(Ok(GrpcHealthProbeResult::Serving)) => {
                    metrics.record_active_health_check(&upstream.name, &peer.url, "healthy");
                    let status = clients.record_active_peer_success(
                        &upstream.name,
                        &peer.url,
                        check.healthy_successes_required,
                    );
                    record_active_transition(
                        &metrics,
                        &upstream.name,
                        &peer.url,
                        status.recovered,
                        "healthy",
                        "healthy_threshold_met",
                    );
                    if status.recovered {
                        tracing::info!(
                            upstream = %upstream.name,
                            peer = %peer.url,
                            path = %check.path,
                            grpc_service = check.grpc_service.as_deref().unwrap_or(""),
                            consecutive_successes = status.consecutive_successes,
                            "active gRPC health check marked peer healthy"
                        );
                    }
                }
                Ok(Ok(GrpcHealthProbeResult::NotServing {
                    http_status,
                    grpc_status,
                    serving_status,
                })) => {
                    let transitioned =
                        clients.record_active_peer_failure(&upstream.name, &peer.url);
                    metrics.record_active_health_check(
                        &upstream.name,
                        &peer.url,
                        "unhealthy_status",
                    );
                    record_active_transition(
                        &metrics,
                        &upstream.name,
                        &peer.url,
                        transitioned,
                        "unhealthy",
                        "unhealthy_status",
                    );
                    tracing::warn!(
                        upstream = %upstream.name,
                        peer = %peer.url,
                        path = %check.path,
                        grpc_service = check.grpc_service.as_deref().unwrap_or(""),
                        status = http_status.as_u16(),
                        grpc_status = grpc_status.as_deref().unwrap_or("-"),
                        serving_status = serving_status.map_or("-", grpc_health_serving_status_label),
                        state = if transitioned { "unhealthy" } else { "still unhealthy" },
                        "active gRPC health check returned an unhealthy status"
                    );
                }
                Ok(Err(error)) => {
                    let transitioned =
                        clients.record_active_peer_failure(&upstream.name, &peer.url);
                    metrics.record_active_health_check(&upstream.name, &peer.url, "request_error");
                    record_active_transition(
                        &metrics,
                        &upstream.name,
                        &peer.url,
                        transitioned,
                        "unhealthy",
                        "request_error",
                    );
                    tracing::warn!(
                        upstream = %upstream.name,
                        peer = %peer.url,
                        path = %check.path,
                        grpc_service = check.grpc_service.as_deref().unwrap_or(""),
                        %error,
                        state = if transitioned { "unhealthy" } else { "still unhealthy" },
                        "active gRPC health check response could not be parsed"
                    );
                }
                Err(_) => {
                    let transitioned =
                        clients.record_active_peer_failure(&upstream.name, &peer.url);
                    metrics.record_active_health_check(&upstream.name, &peer.url, "timeout");
                    record_active_transition(
                        &metrics,
                        &upstream.name,
                        &peer.url,
                        transitioned,
                        "unhealthy",
                        "timeout",
                    );
                    tracing::warn!(
                        upstream = %upstream.name,
                        peer = %peer.url,
                        path = %check.path,
                        grpc_service = check.grpc_service.as_deref().unwrap_or(""),
                        timeout_ms = check.timeout.as_millis() as u64,
                        state = if transitioned { "unhealthy" } else { "still unhealthy" },
                        "active gRPC health check timed out while reading response"
                    );
                }
            }
        }
        Ok(Ok(response)) if response.status().is_success() => {
            metrics.record_active_health_check(&upstream.name, &peer.url, "healthy");
            let status = clients.record_active_peer_success(
                &upstream.name,
                &peer.url,
                check.healthy_successes_required,
            );
            record_active_transition(
                &metrics,
                &upstream.name,
                &peer.url,
                status.recovered,
                "healthy",
                "healthy_threshold_met",
            );
            if status.recovered {
                tracing::info!(
                    upstream = %upstream.name,
                    peer = %peer.url,
                    path = %check.path,
                    consecutive_successes = status.consecutive_successes,
                    "active health check marked peer healthy"
                );
            }
        }
        Ok(Ok(response)) => {
            let transitioned = clients.record_active_peer_failure(&upstream.name, &peer.url);
            metrics.record_active_health_check(&upstream.name, &peer.url, "unhealthy_status");
            record_active_transition(
                &metrics,
                &upstream.name,
                &peer.url,
                transitioned,
                "unhealthy",
                "unhealthy_status",
            );
            tracing::warn!(
                upstream = %upstream.name,
                peer = %peer.url,
                path = %check.path,
                status = response.status().as_u16(),
                state = if transitioned { "unhealthy" } else { "still unhealthy" },
                "active health check returned an unhealthy status"
            );
        }
        Ok(Err(error)) => {
            let transitioned = clients.record_active_peer_failure(&upstream.name, &peer.url);
            metrics.record_active_health_check(&upstream.name, &peer.url, "request_error");
            record_active_transition(
                &metrics,
                &upstream.name,
                &peer.url,
                transitioned,
                "unhealthy",
                "request_error",
            );
            tracing::warn!(
                upstream = %upstream.name,
                peer = %peer.url,
                path = %check.path,
                %error,
                state = if transitioned { "unhealthy" } else { "still unhealthy" },
                "active health check request failed"
            );
        }
        Err(_) => {
            let transitioned = clients.record_active_peer_failure(&upstream.name, &peer.url);
            metrics.record_active_health_check(&upstream.name, &peer.url, "timeout");
            record_active_transition(
                &metrics,
                &upstream.name,
                &peer.url,
                transitioned,
                "unhealthy",
                "timeout",
            );
            tracing::warn!(
                upstream = %upstream.name,
                peer = %peer.url,
                path = %check.path,
                timeout_ms = check.timeout.as_millis() as u64,
                state = if transitioned { "unhealthy" } else { "still unhealthy" },
                "active health check timed out"
            );
        }
    }
}

fn record_active_transition(
    metrics: &Metrics,
    upstream_name: &str,
    peer_url: &str,
    transitioned: bool,
    state: &str,
    reason: &str,
) {
    if transitioned {
        metrics.record_upstream_peer_transition(upstream_name, peer_url, "active", state, reason);
    }
}

fn grpc_health_serving_status_label(status: GrpcHealthServingStatus) -> &'static str {
    match status {
        GrpcHealthServingStatus::Unknown => "UNKNOWN",
        GrpcHealthServingStatus::Serving => "SERVING",
        GrpcHealthServingStatus::NotServing => "NOT_SERVING",
        GrpcHealthServingStatus::ServiceUnknown => "SERVICE_UNKNOWN",
        GrpcHealthServingStatus::Other(_) => "OTHER",
    }
}

pub(super) fn build_active_health_request(
    upstream: &Upstream,
    peer: &UpstreamPeer,
    check: &ActiveHealthCheck,
) -> Result<Request<HttpBody>, Error> {
    let path = &check.path;
    let path: Uri = path.parse().map_err(|error| {
        Error::Server(format!(
            "failed to parse active health-check path `{path}` for peer `{}`: {error}",
            peer.url
        ))
    })?;
    let uri = build_proxy_uri(peer, &path, None).map_err(|error| {
        Error::Server(format!("failed to build active health-check uri: {error}"))
    })?;

    let mut builder = Request::builder().uri(uri).header(HOST, peer.authority.as_str());

    if let Some(service) = check.grpc_service.as_deref() {
        let body = encode_grpc_health_check_request(service);
        builder = builder
            .method(Method::POST)
            .version(Version::HTTP_2)
            .header(CONTENT_TYPE, HeaderValue::from_static("application/grpc"))
            .header(TE, HeaderValue::from_static("trailers"))
            .header(CONTENT_LENGTH, body.len().to_string());
        builder.body(full_body(body)).map_err(|error| {
            Error::Server(format!("failed to build active health-check request: {error}"))
        })
    } else {
        builder
            .method(Method::GET)
            .version(upstream_request_version(upstream.protocol))
            .body(full_body(Bytes::new()))
            .map_err(|error| {
                Error::Server(format!("failed to build active health-check request: {error}"))
            })
    }
}
