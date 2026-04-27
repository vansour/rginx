use super::*;

#[derive(Debug, Clone)]
pub struct DownstreamRequestOptions {
    pub request_body_read_timeout: Option<Duration>,
    pub max_request_body_bytes: Option<usize>,
    pub request_buffering: RouteBufferingPolicy,
    pub streaming_response_idle_timeout: Option<Duration>,
    pub cache: Option<rginx_core::RouteCachePolicy>,
}

#[derive(Debug, Clone)]
pub struct DownstreamRequestContext<'a> {
    pub listener_id: &'a str,
    pub downstream_proto: &'a str,
    pub request_id: &'a str,
    pub options: DownstreamRequestOptions,
}
