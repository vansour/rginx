use super::*;
use crate::proxy::clients::ProxyClient;

pub(super) struct PreparedForwardRequest {
    pub(super) request_headers: HeaderMap,
    pub(super) response_idle_timeout: Duration,
    pub(super) grpc_web_mode: Option<GrpcWebMode>,
    pub(super) upstream_request_timeout: Duration,
    pub(super) client: ProxyClient,
    pub(super) downstream_upgrade: Option<OnUpgrade>,
    pub(super) prepared_request: PreparedProxyRequest,
    pub(super) peers: Vec<ResolvedUpstreamPeer>,
}

pub(super) async fn prepare_forward_request(
    state: &SharedState,
    clients: &ProxyClients,
    mut request: Request<HttpBody>,
    target: &ProxyTarget,
    client_address: &ClientAddress,
    downstream: &DownstreamRequestContext<'_>,
) -> Result<PreparedForwardRequest, HttpResponse> {
    let request_headers = request.headers().clone();
    let response_idle_timeout =
        downstream.options.streaming_response_idle_timeout.unwrap_or(target.upstream.idle_timeout);
    state.record_upstream_request(&target.upstream_name);
    let grpc_web_mode = match detect_grpc_web_mode(request.headers()) {
        Ok(mode) => mode,
        Err(message) => {
            state.record_upstream_unsupported_media_type_response(&target.upstream_name);
            return Err(unsupported_media_type(&request_headers, format!("{message}\n")));
        }
    };
    let upstream_request_timeout =
        match effective_upstream_request_timeout(&request_headers, target.upstream.request_timeout)
        {
            Ok(timeout) => timeout,
            Err(message) => {
                state.record_upstream_bad_request_response(&target.upstream_name);
                return Err(bad_request(&request_headers, format!("{message}\n")));
            }
        };
    let client = match clients.for_upstream(target.upstream.as_ref()) {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(
                request_id = %downstream.request_id,
                upstream = %target.upstream_name,
                upstream_sni_enabled = target.upstream.server_name,
                upstream_server_name = target.upstream.server_name_override.as_deref().unwrap_or("-"),
                upstream_verify = super::upstream_tls_verify_label(&target.upstream.tls),
                upstream_tls_failure = super::classify_upstream_tls_failure(&error),
                %error,
                "failed to select proxy client"
            );
            state.record_upstream_bad_gateway_response(&target.upstream_name);
            return Err(bad_gateway(
                &request_headers,
                format!("upstream `{}` TLS client is unavailable\n", target.upstream_name),
            ));
        }
    };

    let downstream_upgrade = if is_upgrade_request(request.version(), request.headers()) {
        Some(hyper::upgrade::on(&mut request))
    } else {
        None
    };
    if downstream_upgrade.is_some() && target.upstream.protocol == UpstreamProtocol::Http3 {
        state.record_upstream_bad_gateway_response(&target.upstream_name);
        return Err(bad_gateway(
            &request_headers,
            format!("upstream `{}` does not support upgrade over http3\n", target.upstream_name),
        ));
    }

    let prepared_request = match PreparedProxyRequest::from_request(
        request,
        &target.upstream_name,
        downstream.options.request_body_read_timeout,
        target.upstream.write_timeout,
        target.upstream.max_replayable_request_body_bytes,
        downstream.options.max_request_body_bytes,
        downstream.options.request_buffering,
        grpc_web_mode.as_ref(),
    )
    .await
    {
        Ok(request) => request,
        Err(PrepareRequestError::PayloadTooLarge { max_request_body_bytes }) => {
            tracing::info!(
                request_id = %downstream.request_id,
                upstream = %target.upstream_name,
                max_request_body_bytes,
                "rejecting request body that exceeds configured server limit"
            );
            state.record_upstream_payload_too_large_response(&target.upstream_name);
            return Err(payload_too_large(
                &request_headers,
                format!(
                    "request body exceeds server.max_request_body_bytes ({max_request_body_bytes} bytes)\n"
                ),
            ));
        }
        Err(error) => {
            tracing::warn!(
                request_id = %downstream.request_id,
                upstream = %target.upstream_name,
                %error,
                "failed to prepare upstream request"
            );
            if invalid_downstream_request_body_error(&error) {
                state.record_upstream_bad_request_response(&target.upstream_name);
                return Err(bad_request(
                    &request_headers,
                    format!("invalid downstream request body: {error}\n"),
                ));
            }
            state.record_upstream_bad_gateway_response(&target.upstream_name);
            return Err(bad_gateway(
                &request_headers,
                format!("failed to prepare upstream request for `{}`\n", target.upstream_name),
            ));
        }
    };
    let can_failover = prepared_request.can_failover();
    let selected = clients
        .select_peers(
            target.upstream.as_ref(),
            client_address.client_ip,
            if can_failover { MAX_FAILOVER_ATTEMPTS } else { 1 },
        )
        .await;
    let peers = selected.peers;
    if peers.is_empty() {
        tracing::warn!(
            request_id = %downstream.request_id,
            upstream = %target.upstream_name,
            skipped_unhealthy = selected.skipped_unhealthy,
            "proxy route has no healthy peers available"
        );
        state.record_upstream_no_healthy_peers(&target.upstream_name);
        state.record_upstream_bad_gateway_response(&target.upstream_name);
        return Err(bad_gateway(
            &request_headers,
            format!("upstream `{}` has no healthy peers available\n", target.upstream_name),
        ));
    }

    Ok(PreparedForwardRequest {
        request_headers,
        response_idle_timeout,
        grpc_web_mode,
        upstream_request_timeout,
        client,
        downstream_upgrade,
        prepared_request,
        peers,
    })
}
