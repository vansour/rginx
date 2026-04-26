use super::clients::ProxyClients;
use super::*;

pub async fn probe_upstream_peer(
    clients: ProxyClients,
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
            let level = if transitioned { "unhealthy" } else { "still unhealthy" };
            tracing::warn!(
                upstream = %upstream.name,
                peer = %peer.url,
                upstream_sni_enabled = upstream.server_name,
                upstream_server_name = upstream.server_name_override.as_deref().unwrap_or("-"),
                upstream_verify = super::upstream_tls_verify_label(&upstream.tls),
                upstream_tls_failure = super::classify_upstream_tls_failure(&error),
                %error,
                state = level,
                "active health check could not acquire a proxy client"
            );
            return;
        }
    };
    let resolved_peers = match client.resolve_peer(&peer).await {
        Ok(peers) if !peers.is_empty() => peers,
        Ok(_) => {
            tracing::warn!(
                upstream = %upstream.name,
                peer = %peer.url,
                path = %check.path,
                "active health check has no resolved endpoints to probe"
            );
            return;
        }
        Err(error) => {
            tracing::warn!(
                upstream = %upstream.name,
                peer = %peer.url,
                path = %check.path,
                %error,
                "active health check failed to resolve upstream endpoints"
            );
            return;
        }
    };

    for resolved_peer in resolved_peers {
        let request = match super::build_active_health_request(
            upstream.as_ref(),
            &resolved_peer,
            check,
        ) {
            Ok(request) => request,
            Err(error) => {
                let transitioned =
                    clients.record_active_peer_failure(&upstream.name, &resolved_peer.endpoint_key);
                let level = if transitioned { "unhealthy" } else { "still unhealthy" };
                tracing::warn!(
                    upstream = %upstream.name,
                    peer = %resolved_peer.display_url,
                    logical_peer = %peer.url,
                    path = %check.path,
                    upstream_sni_enabled = upstream.server_name,
                    upstream_server_name = upstream.server_name_override.as_deref().unwrap_or("-"),
                    upstream_verify = super::upstream_tls_verify_label(&upstream.tls),
                    upstream_tls_failure = super::classify_upstream_tls_failure(&error),
                    %error,
                    state = level,
                    "active health check request could not be built"
                );
                continue;
            }
        };

        match tokio::time::timeout(
            check.timeout,
            client.request(upstream.as_ref(), &resolved_peer, request),
        )
        .await
        {
            Ok(Ok(response)) if check.grpc_service.is_some() => {
                match tokio::time::timeout(
                    check.timeout,
                    evaluate_grpc_health_probe_response(response),
                )
                .await
                {
                    Ok(Ok(GrpcHealthProbeResult::Serving)) => {
                        let status = clients.record_active_peer_success(
                            &upstream.name,
                            &resolved_peer.endpoint_key,
                            check.healthy_successes_required,
                        );
                        if status.recovered {
                            tracing::info!(
                                upstream = %upstream.name,
                                peer = %resolved_peer.display_url,
                                logical_peer = %peer.url,
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
                        let transitioned = clients.record_active_peer_failure(
                            &upstream.name,
                            &resolved_peer.endpoint_key,
                        );
                        tracing::warn!(
                            upstream = %upstream.name,
                            peer = %resolved_peer.display_url,
                            logical_peer = %peer.url,
                            path = %check.path,
                            grpc_service = check.grpc_service.as_deref().unwrap_or(""),
                            upstream_sni_enabled = upstream.server_name,
                            upstream_server_name = upstream.server_name_override.as_deref().unwrap_or("-"),
                            upstream_verify = super::upstream_tls_verify_label(&upstream.tls),
                            status = http_status.as_u16(),
                            grpc_status = grpc_status.as_deref().unwrap_or("-"),
                            serving_status = serving_status.map_or("-", grpc_health_serving_status_label),
                            state = if transitioned { "unhealthy" } else { "still unhealthy" },
                            "active gRPC health check returned an unhealthy status"
                        );
                    }
                    Ok(Err(error)) => {
                        let transitioned = clients.record_active_peer_failure(
                            &upstream.name,
                            &resolved_peer.endpoint_key,
                        );
                        tracing::warn!(
                            upstream = %upstream.name,
                            peer = %resolved_peer.display_url,
                            logical_peer = %peer.url,
                            path = %check.path,
                            grpc_service = check.grpc_service.as_deref().unwrap_or(""),
                            upstream_sni_enabled = upstream.server_name,
                            upstream_server_name = upstream.server_name_override.as_deref().unwrap_or("-"),
                            upstream_verify = super::upstream_tls_verify_label(&upstream.tls),
                            upstream_tls_failure = super::classify_upstream_tls_failure(&error),
                            %error,
                            state = if transitioned { "unhealthy" } else { "still unhealthy" },
                            "active gRPC health check response could not be parsed"
                        );
                    }
                    Err(_) => {
                        let transitioned = clients.record_active_peer_failure(
                            &upstream.name,
                            &resolved_peer.endpoint_key,
                        );
                        tracing::warn!(
                            upstream = %upstream.name,
                            peer = %resolved_peer.display_url,
                            logical_peer = %peer.url,
                            path = %check.path,
                            grpc_service = check.grpc_service.as_deref().unwrap_or(""),
                            upstream_sni_enabled = upstream.server_name,
                            upstream_server_name = upstream.server_name_override.as_deref().unwrap_or("-"),
                            upstream_verify = super::upstream_tls_verify_label(&upstream.tls),
                            timeout_ms = check.timeout.as_millis() as u64,
                            state = if transitioned { "unhealthy" } else { "still unhealthy" },
                            "active gRPC health check timed out while reading response"
                        );
                    }
                }
            }
            Ok(Ok(response)) if response.status().is_success() => {
                let status = clients.record_active_peer_success(
                    &upstream.name,
                    &resolved_peer.endpoint_key,
                    check.healthy_successes_required,
                );
                if status.recovered {
                    tracing::info!(
                        upstream = %upstream.name,
                        peer = %resolved_peer.display_url,
                        logical_peer = %peer.url,
                        path = %check.path,
                        consecutive_successes = status.consecutive_successes,
                        "active health check marked peer healthy"
                    );
                }
            }
            Ok(Ok(response)) => {
                let transitioned =
                    clients.record_active_peer_failure(&upstream.name, &resolved_peer.endpoint_key);
                tracing::warn!(
                    upstream = %upstream.name,
                    peer = %resolved_peer.display_url,
                    logical_peer = %peer.url,
                    path = %check.path,
                    upstream_sni_enabled = upstream.server_name,
                    upstream_server_name = upstream.server_name_override.as_deref().unwrap_or("-"),
                    upstream_verify = super::upstream_tls_verify_label(&upstream.tls),
                    status = response.status().as_u16(),
                    state = if transitioned { "unhealthy" } else { "still unhealthy" },
                    "active health check returned an unhealthy status"
                );
            }
            Ok(Err(error)) => {
                let transitioned =
                    clients.record_active_peer_failure(&upstream.name, &resolved_peer.endpoint_key);
                tracing::warn!(
                    upstream = %upstream.name,
                    peer = %resolved_peer.display_url,
                    logical_peer = %peer.url,
                    path = %check.path,
                    upstream_sni_enabled = upstream.server_name,
                    upstream_server_name = upstream.server_name_override.as_deref().unwrap_or("-"),
                    upstream_verify = super::upstream_tls_verify_label(&upstream.tls),
                    upstream_tls_failure = super::classify_upstream_tls_failure(&error),
                    %error,
                    state = if transitioned { "unhealthy" } else { "still unhealthy" },
                    "active health check request failed"
                );
            }
            Err(_) => {
                let transitioned =
                    clients.record_active_peer_failure(&upstream.name, &resolved_peer.endpoint_key);
                tracing::warn!(
                    upstream = %upstream.name,
                    peer = %resolved_peer.display_url,
                    logical_peer = %peer.url,
                    path = %check.path,
                    upstream_sni_enabled = upstream.server_name,
                    upstream_server_name = upstream.server_name_override.as_deref().unwrap_or("-"),
                    upstream_verify = super::upstream_tls_verify_label(&upstream.tls),
                    timeout_ms = check.timeout.as_millis() as u64,
                    state = if transitioned { "unhealthy" } else { "still unhealthy" },
                    "active health check timed out"
                );
            }
        }
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
