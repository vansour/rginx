use super::*;

pub(super) fn log_successful_attempt(
    target: &ProxyTarget,
    peer: &ResolvedUpstreamPeer,
    downstream: &DownstreamRequestContext<'_>,
    attempt_index: usize,
    recovered: bool,
) {
    if recovered {
        tracing::info!(
            upstream = %target.upstream_name,
            peer = %peer.display_url,
            logical_peer = %peer.logical_peer_url,
            "upstream peer recovered from passive health check cooldown"
        );
    }
    if attempt_index > 0 {
        tracing::info!(
            request_id = %downstream.request_id,
            upstream = %target.upstream_name,
            peer = %peer.display_url,
            logical_peer = %peer.logical_peer_url,
            attempt = attempt_index + 1,
            "upstream failover request succeeded"
        );
    }
}
