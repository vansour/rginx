use super::*;

pub(super) fn failed_to_build_request_response<E: std::fmt::Display>(
    state: &SharedState,
    request_headers: &HeaderMap,
    target: &ProxyTarget,
    peer: &ResolvedUpstreamPeer,
    downstream: &DownstreamRequestContext<'_>,
    error: E,
    cache: &ForwardCacheContext,
) -> HttpResponse {
    tracing::warn!(
        request_id = %downstream.request_id,
        upstream = %target.upstream_name,
        peer = %peer.display_url,
        logical_peer = %peer.logical_peer_url,
        upstream_sni_enabled = target.upstream.server_name,
        upstream_server_name = target.upstream.server_name_override.as_deref().unwrap_or("-"),
        upstream_verify = super::upstream_tls_verify_label(&target.upstream.tls),
        %error,
        "failed to build upstream request"
    );
    state.record_upstream_bad_gateway_response(&target.upstream_name);
    cache.mark_response(bad_gateway(
        request_headers,
        format!("failed to build upstream request for `{}`\n", target.upstream_name),
    ))
}

pub(super) fn upstream_unavailable_response(
    state: &SharedState,
    request_headers: &HeaderMap,
    target: &ProxyTarget,
    cache: &ForwardCacheContext,
) -> HttpResponse {
    state.record_upstream_bad_gateway_response(&target.upstream_name);
    cache.mark_response(bad_gateway(
        request_headers,
        format!("upstream `{}` is unavailable\n", target.upstream_name),
    ))
}
