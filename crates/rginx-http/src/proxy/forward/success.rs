use super::*;

pub(super) struct UpstreamSuccessContext<'a> {
    pub(super) state: &'a SharedState,
    pub(super) downstream_upgrade: Option<OnUpgrade>,
    pub(super) listener_id: &'a str,
    pub(super) target: &'a ProxyTarget,
    pub(super) peer: &'a ResolvedUpstreamPeer,
    pub(super) active_peer: super::super::health::ActivePeerGuard,
    pub(super) response_idle_timeout: Duration,
    pub(super) grpc_response_deadline: Option<super::response::GrpcResponseDeadline>,
    pub(super) grpc_web_mode: Option<&'a GrpcWebMode>,
}

pub(super) fn finalize_upstream_success(
    mut response: Response<HttpBody>,
    context: UpstreamSuccessContext<'_>,
) -> HttpResponse {
    let upstream_upgrade = if context.downstream_upgrade.is_some()
        && is_upgrade_response(response.status(), response.headers())
    {
        Some(hyper::upgrade::on(&mut response))
    } else {
        None
    };

    if let (Some(downstream_upgrade), Some(upstream_upgrade)) =
        (context.downstream_upgrade, upstream_upgrade)
    {
        let connection_guard = context.state.retain_connection_slot(context.listener_id);
        context.state.spawn_background_task(proxy_upgraded_connection(
            downstream_upgrade,
            upstream_upgrade,
            context.target.upstream_name.clone(),
            context.peer.display_url.clone(),
            context.active_peer,
            connection_guard,
        ));
        return build_downstream_response(
            response,
            &context.target.upstream_name,
            &context.peer.display_url,
            context.response_idle_timeout,
            context.grpc_response_deadline,
            context.grpc_web_mode,
            None,
        );
    }

    build_downstream_response(
        response,
        &context.target.upstream_name,
        &context.peer.display_url,
        context.response_idle_timeout,
        context.grpc_response_deadline,
        context.grpc_web_mode,
        Some(context.active_peer),
    )
}
