use super::super::codec::{
    ParsedGrpcWebRequestFrame, decode_grpc_web_request_frame, invalid_grpc_web_body,
};
use super::super::*;

pin_project! {
    pub(crate) struct GrpcWebRequestBody<B> {
        #[pin]
        inner: B,
        inner_finished: bool,
        inner_trailers: Option<HeaderMap>,
        pending_data: Option<Bytes>,
        pending_trailers: Option<HeaderMap>,
        buffer: BytesMut,
        saw_grpc_web_trailers: bool,
    }
}

impl<B> GrpcWebRequestBody<B> {
    pub(crate) fn new(inner: B) -> Self {
        Self {
            inner,
            inner_finished: false,
            inner_trailers: None,
            pending_data: None,
            pending_trailers: None,
            buffer: BytesMut::new(),
            saw_grpc_web_trailers: false,
        }
    }
}

impl<B> hyper::body::Body for GrpcWebRequestBody<B>
where
    B: hyper::body::Body<Data = Bytes>,
    B::Error: Into<BoxError>,
{
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();

        loop {
            if let Some(data) = this.pending_data.take() {
                return std::task::Poll::Ready(Some(Ok(Frame::data(data))));
            }

            if let Some(trailers) = this.pending_trailers.take() {
                return std::task::Poll::Ready(Some(Ok(Frame::trailers(trailers))));
            }

            match decode_grpc_web_request_frame(
                this.buffer,
                *this.inner_finished,
                *this.saw_grpc_web_trailers,
            ) {
                Ok(Some(ParsedGrpcWebRequestFrame::Data(data))) => {
                    return std::task::Poll::Ready(Some(Ok(Frame::data(data))));
                }
                Ok(Some(ParsedGrpcWebRequestFrame::Trailers(mut trailers))) => {
                    *this.saw_grpc_web_trailers = true;
                    if let Some(inner_trailers) = this.inner_trailers.take() {
                        append_header_map(&mut trailers, &inner_trailers);
                    }
                    if trailers.is_empty() {
                        continue;
                    }
                    return std::task::Poll::Ready(Some(Ok(Frame::trailers(trailers))));
                }
                Ok(None) => {}
                Err(error) => return std::task::Poll::Ready(Some(Err(error))),
            }

            if *this.inner_finished {
                if let Some(trailers) = this.inner_trailers.take() {
                    return std::task::Poll::Ready(Some(Ok(Frame::trailers(trailers))));
                }
                return std::task::Poll::Ready(None);
            }

            match this.inner.as_mut().poll_frame(cx) {
                std::task::Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
                    Ok(data) => {
                        if *this.saw_grpc_web_trailers && !data.is_empty() {
                            return std::task::Poll::Ready(Some(Err(invalid_grpc_web_body(
                                "grpc-web request trailer frame must be terminal",
                            ))));
                        }
                        this.buffer.extend_from_slice(&data);
                    }
                    Err(frame) => match frame.into_trailers() {
                        Ok(trailers) => {
                            if let Some(existing) = this.inner_trailers.as_mut() {
                                append_header_map(existing, &trailers);
                            } else {
                                *this.inner_trailers = Some(trailers);
                            }
                            *this.inner_finished = true;
                        }
                        Err(_) => {
                            return std::task::Poll::Ready(Some(Err(invalid_grpc_web_body(
                                "unexpected downstream request frame for grpc-web decoding",
                            ))));
                        }
                    },
                },
                std::task::Poll::Ready(Some(Err(error))) => {
                    *this.inner_finished = true;
                    return std::task::Poll::Ready(Some(Err(error.into())));
                }
                std::task::Poll::Ready(None) => {
                    *this.inner_finished = true;
                }
                std::task::Poll::Pending => return std::task::Poll::Pending,
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner_finished
            && self.inner_trailers.is_none()
            && self.pending_data.is_none()
            && self.pending_trailers.is_none()
            && self.buffer.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}
