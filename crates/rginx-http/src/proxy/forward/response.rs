use super::super::grpc_web::{
    GrpcWebMode, GrpcWebResponseBody, GrpcWebTextEncodeBody, extract_grpc_initial_trailers,
};
use super::health::{ActivePeerBody, ActivePeerGuard};
use super::*;

#[derive(Debug, Clone)]
pub(super) struct GrpcResponseDeadline {
    pub(super) deadline: TokioInstant,
    pub(super) timeout: Duration,
    pub(super) timeout_message: String,
}

pub(super) fn build_downstream_response(
    response: Response<Incoming>,
    upstream_name: &str,
    peer_url: &str,
    idle_timeout: Duration,
    grpc_response_deadline: Option<GrpcResponseDeadline>,
    grpc_web_mode: Option<&GrpcWebMode>,
    active_peer: Option<ActivePeerGuard>,
) -> HttpResponse {
    let (mut parts, body) = response.into_parts();
    let preserve_upgrade =
        grpc_web_mode.is_none() && is_upgrade_response(parts.status, &parts.headers);
    sanitize_response_headers(&mut parts.headers, preserve_upgrade);

    let label = format!("upstream `{upstream_name}` response body from `{peer_url}`");
    let body = if preserve_upgrade {
        full_body(Bytes::new())
    } else if let Some(grpc_web_mode) = grpc_web_mode {
        let fallback_trailers = extract_grpc_initial_trailers(&mut parts.headers);
        parts.headers.insert(CONTENT_TYPE, grpc_web_mode.downstream_content_type.clone());
        parts.headers.remove(CONTENT_LENGTH);
        let body =
            wrap_stream_timeout_pipeline(body, idle_timeout, label.clone(), grpc_response_deadline);
        let body = GrpcWebResponseBody::new(
            ActivePeerBody::new(
                body,
                active_peer.expect("non-upgrade responses should track peer activity"),
            ),
            fallback_trailers,
        );
        if grpc_web_mode.is_text() {
            GrpcWebTextEncodeBody::new(body).boxed_unsync()
        } else {
            body.boxed_unsync()
        }
    } else {
        let body = wrap_stream_timeout_pipeline(body, idle_timeout, label, grpc_response_deadline);
        ActivePeerBody::new(
            body,
            active_peer.expect("non-upgrade responses should track peer activity"),
        )
        .boxed_unsync()
    };

    Response::from_parts(parts, body)
}

fn wrap_stream_timeout_pipeline<B>(
    body: B,
    idle_timeout: Duration,
    label: String,
    grpc_response_deadline: Option<GrpcResponseDeadline>,
) -> HttpBody
where
    B: hyper::body::Body<Data = Bytes> + Send + 'static,
    B::Error: Into<BoxError> + 'static,
{
    // Order matters:
    // 1. idle timeout protects stalled upstream body progress
    // 2. optional grpc deadline converts an overrun into terminal gRPC trailers
    // 3. outer layers such as ActivePeerBody / grpc-web translation run afterward
    let body = IdleTimeoutBody::new(body, idle_timeout, label.clone());
    if let Some(deadline) = grpc_response_deadline {
        GrpcDeadlineBody::new(
            body,
            deadline.deadline,
            deadline.timeout,
            label,
            deadline.timeout_message,
        )
        .boxed_unsync()
    } else {
        body.boxed_unsync()
    }
}
