use super::*;

pub(super) async fn finalize_streaming_request_body(
    body_completion: Option<StreamingBodyCompletion>,
    state: &SharedState,
    request_headers: &HeaderMap,
    target: &ProxyTarget,
    peer: &ResolvedUpstreamPeer,
    downstream: DownstreamRequestContext<'_>,
) -> Result<(), HttpResponse> {
    let Some(body_completion) = body_completion else {
        return Ok(());
    };

    let result: Result<(), BoxError> = match body_completion.await {
        Ok(result) => result,
        Err(_) => {
            return Err(bad_gateway(
                request_headers,
                format!(
                    "failed to finalize streamed request body for upstream `{}`\n",
                    target.upstream_name,
                ),
            ));
        }
    };

    match result {
        Ok(()) => Ok(()),
        Err(error) => {
            if let Some(max_request_body_bytes) = downstream_request_body_limit(error.as_ref()) {
                tracing::info!(
                    request_id = %downstream.request_id,
                    upstream = %target.upstream_name,
                    peer = %peer.display_url,
                    logical_peer = %peer.logical_peer_url,
                    max_request_body_bytes,
                    %error,
                    "rejecting streamed request body that exceeds configured server limit after upstream response"
                );
                state.record_upstream_payload_too_large_response(&target.upstream_name);
                return Err(payload_too_large(
                    request_headers,
                    format!(
                        "request body exceeds server.max_request_body_bytes ({max_request_body_bytes} bytes)\n"
                    ),
                ));
            }

            if invalid_downstream_request_body_error(error.as_ref()) {
                tracing::warn!(
                    request_id = %downstream.request_id,
                    upstream = %target.upstream_name,
                    peer = %peer.display_url,
                    logical_peer = %peer.logical_peer_url,
                    %error,
                    "downstream request body became invalid after upstream response"
                );
                state.record_upstream_bad_request_response(&target.upstream_name);
                return Err(bad_request(
                    request_headers,
                    format!("invalid downstream request body: {error}\n"),
                ));
            }

            tracing::warn!(
                request_id = %downstream.request_id,
                upstream = %target.upstream_name,
                peer = %peer.display_url,
                logical_peer = %peer.logical_peer_url,
                %error,
                "streamed request body failed after upstream response"
            );
            state.record_upstream_bad_gateway_response(&target.upstream_name);
            Err(bad_gateway(
                request_headers,
                format!("upstream `{}` is unavailable\n", target.upstream_name),
            ))
        }
    }
}
