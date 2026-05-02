use bytes::Bytes;
use http::StatusCode;
use hyper::body::{Body, Frame, SizeHint};
use pin_project_lite::pin_project;

use super::super::entry::DownstreamRangeTrimPlan;
use super::*;
use crate::handler::{BoxError, boxed_body};

pin_project! {
    pub(super) struct DownstreamRangeTrimBody<B> {
        #[pin]
        inner: B,
        skip_bytes: usize,
        emit_bytes: usize,
        done: bool,
    }
}

impl<B> DownstreamRangeTrimBody<B> {
    pub(super) fn new(inner: B, plan: DownstreamRangeTrimPlan) -> Self {
        Self { inner, skip_bytes: plan.skip_bytes(), emit_bytes: plan.emit_bytes(), done: false }
    }
}

impl<B> Body for DownstreamRangeTrimBody<B>
where
    B: Body<Data = Bytes>,
    B::Error: Into<BoxError> + 'static,
{
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();

        if *this.done {
            return std::task::Poll::Ready(None);
        }
        if *this.emit_bytes == 0 {
            *this.done = true;
            return std::task::Poll::Ready(None);
        }

        loop {
            match this.inner.as_mut().poll_frame(cx) {
                std::task::Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
                    Ok(mut data) => {
                        if *this.skip_bytes >= data.len() {
                            *this.skip_bytes -= data.len();
                            continue;
                        }
                        if *this.skip_bytes > 0 {
                            data = data.slice(*this.skip_bytes..);
                            *this.skip_bytes = 0;
                        }
                        if *this.emit_bytes == 0 {
                            continue;
                        }
                        if data.len() > *this.emit_bytes {
                            data = data.slice(..*this.emit_bytes);
                        }
                        *this.emit_bytes -= data.len();
                        if *this.emit_bytes == 0 {
                            *this.done = true;
                        }
                        if data.is_empty() {
                            continue;
                        }
                        return std::task::Poll::Ready(Some(Ok(Frame::data(data))));
                    }
                    Err(frame) => match frame.into_trailers() {
                        Ok(trailers) => {
                            *this.done = true;
                            if *this.skip_bytes > 0 || *this.emit_bytes > 0 {
                                return std::task::Poll::Ready(Some(Err(truncated_range_error())));
                            }
                            return std::task::Poll::Ready(Some(Ok(Frame::trailers(trailers))));
                        }
                        Err(frame) => return std::task::Poll::Ready(Some(Ok(frame))),
                    },
                },
                std::task::Poll::Ready(Some(Err(error))) => {
                    *this.done = true;
                    return std::task::Poll::Ready(Some(Err(error.into())));
                }
                std::task::Poll::Ready(None) => {
                    *this.done = true;
                    if *this.skip_bytes > 0 || *this.emit_bytes > 0 {
                        return std::task::Poll::Ready(Some(Err(truncated_range_error())));
                    }
                    return std::task::Poll::Ready(None);
                }
                std::task::Poll::Pending => return std::task::Poll::Pending,
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.done
    }

    fn size_hint(&self) -> SizeHint {
        let mut hint = SizeHint::default();
        hint.set_exact(self.emit_bytes as u64);
        hint
    }
}

fn truncated_range_error() -> BoxError {
    std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "streamed range response ended before the requested subrange was fully produced",
    )
    .into()
}

pub(in crate::cache) fn build_downstream_response<B>(
    mut parts: http::response::Parts,
    body: B,
    trim_plan: Option<DownstreamRangeTrimPlan>,
) -> HttpResponse
where
    B: Body<Data = Bytes> + Send + 'static,
    B::Error: Into<BoxError> + 'static,
{
    match trim_plan {
        Some(plan) => {
            parts.status = StatusCode::PARTIAL_CONTENT;
            parts.headers = plan.headers().clone();
            http::Response::from_parts(parts, boxed_body(DownstreamRangeTrimBody::new(body, plan)))
        }
        None => http::Response::from_parts(parts, boxed_body(body)),
    }
}
