use bytes::Bytes;
use http::Response;
use http::header::{HeaderMap, HeaderName};
use http_body_util::BodyExt;
use hyper::body::Frame;
use hyper::body::SizeHint;
use rginx_core::AccessLogFormat;

use crate::handler::access_log::{OwnedAccessLogContext, log_access_event};
use crate::handler::{BoxError, HttpBody, HttpResponse};
use crate::state::SharedState;

use super::GrpcStatusCode;
use super::grpc_web::GrpcWebObservabilityParser;
use super::metadata::GrpcRequestMetadata;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::handler) struct GrpcObservability {
    pub(crate) protocol: String,
    pub(crate) service: String,
    pub(crate) method: String,
    pub(crate) status: Option<String>,
    pub(crate) message: Option<String>,
}

pub(in crate::handler) fn grpc_observability(
    metadata: Option<GrpcRequestMetadata<'_>>,
    response_headers: &HeaderMap,
) -> Option<GrpcObservability> {
    let metadata = metadata?;
    let mut grpc = GrpcObservability {
        protocol: metadata.protocol.to_string(),
        service: metadata.service.to_string(),
        method: metadata.method.to_string(),
        status: None,
        message: None,
    };
    grpc.update_from_headers(response_headers);
    Some(grpc)
}

impl GrpcObservability {
    pub(in crate::handler) fn update_from_headers(&mut self, headers: &HeaderMap) {
        if let Some(status) =
            crate::handler::dispatch::header_value(headers, HeaderName::from_static("grpc-status"))
        {
            self.status = Some(status.to_string());
        }
        if let Some(message) =
            crate::handler::dispatch::header_value(headers, HeaderName::from_static("grpc-message"))
        {
            self.message = Some(message.to_string());
        }
    }
}

#[derive(Clone)]
pub(in crate::handler) struct GrpcStatsContext {
    pub state: SharedState,
    pub listener_id: String,
    pub vhost_id: String,
    pub route_id: Option<String>,
}

struct GrpcResponseFinalizer {
    format: Option<AccessLogFormat>,
    context: OwnedAccessLogContext,
    stats: Option<GrpcStatsContext>,
    finalized: bool,
}

impl GrpcResponseFinalizer {
    fn new(
        format: Option<AccessLogFormat>,
        context: OwnedAccessLogContext,
        stats: Option<GrpcStatsContext>,
    ) -> Self {
        Self { format, context, stats, finalized: false }
    }

    fn finalize(&mut self, grpc: &GrpcObservability) {
        if self.finalized {
            return;
        }
        self.finalized = true;

        if let Some(stats) = &self.stats {
            stats.state.record_grpc_status(
                &stats.listener_id,
                &stats.vhost_id,
                stats.route_id.as_deref(),
                grpc.status.as_deref(),
            );
        }
        log_access_event(self.format.as_ref(), self.context.as_borrowed(Some(grpc)));
    }
}

struct GrpcAccessLogBody {
    inner: HttpBody,
    finalizer: GrpcResponseFinalizer,
    grpc: GrpcObservability,
    grpc_web: Option<GrpcWebObservabilityParser>,
    stream_completed: bool,
}

impl GrpcAccessLogBody {
    fn new(inner: HttpBody, finalizer: GrpcResponseFinalizer, grpc: GrpcObservability) -> Self {
        let grpc_web = GrpcWebObservabilityParser::for_protocol(&grpc.protocol);
        Self { inner, finalizer, grpc, grpc_web, stream_completed: false }
    }

    fn observe_frame(&mut self, frame: &Frame<Bytes>) {
        if let Some(trailers) = frame.trailers_ref() {
            self.grpc.update_from_headers(trailers);
        }

        if let Some(data) = frame.data_ref()
            && let Some(parser) = self.grpc_web.as_mut()
        {
            parser.observe_chunk(data, &mut self.grpc);
        }
    }

    fn finish(&mut self) {
        if let Some(parser) = self.grpc_web.as_mut() {
            parser.finish(&mut self.grpc);
        }
        self.finalizer.finalize(&self.grpc);
    }

    fn mark_cancelled_if_dropped_early(&mut self) {
        if self.stream_completed || self.grpc.status.is_some() {
            return;
        }

        self.grpc.status = Some(GrpcStatusCode::Cancelled.as_str().to_string());
        if self.grpc.message.is_none() {
            self.grpc.message = Some("downstream cancelled".to_string());
        }
    }
}

impl Drop for GrpcAccessLogBody {
    fn drop(&mut self) {
        self.mark_cancelled_if_dropped_early();
        self.finish();
    }
}

impl hyper::body::Body for GrpcAccessLogBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match std::pin::Pin::new(&mut self.inner).poll_frame(cx) {
            std::task::Poll::Ready(Some(Ok(frame))) => {
                let stream_completed = frame.is_trailers() || self.inner.is_end_stream();
                self.observe_frame(&frame);
                if stream_completed {
                    self.stream_completed = true;
                    self.finish();
                }
                std::task::Poll::Ready(Some(Ok(frame)))
            }
            std::task::Poll::Ready(Some(Err(error))) => {
                self.stream_completed = true;
                self.finish();
                std::task::Poll::Ready(Some(Err(error)))
            }
            std::task::Poll::Ready(None) => {
                self.stream_completed = true;
                self.finish();
                std::task::Poll::Ready(None)
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

pub(in crate::handler) fn wrap_grpc_observability_response(
    response: HttpResponse,
    format: Option<AccessLogFormat>,
    context: OwnedAccessLogContext,
    grpc: GrpcObservability,
    stats: Option<GrpcStatsContext>,
) -> HttpResponse {
    let (parts, body) = response.into_parts();
    let body =
        GrpcAccessLogBody::new(body, GrpcResponseFinalizer::new(format, context, stats), grpc)
            .boxed_unsync();
    Response::from_parts(parts, body)
}
