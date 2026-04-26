use super::super::codec::{
    decode_grpc_web_text_chunk, decode_grpc_web_text_final, invalid_grpc_web_body,
};
use super::super::*;

pin_project! {
    pub(crate) struct GrpcWebTextDecodeBody<B> {
        #[pin]
        inner: B,
        pending_data: Option<Bytes>,
        pending_trailers: Option<HeaderMap>,
        carryover: BytesMut,
        inner_finished: bool,
    }
}

impl<B> GrpcWebTextDecodeBody<B> {
    pub(crate) fn new(inner: B) -> Self {
        Self {
            inner,
            pending_data: None,
            pending_trailers: None,
            carryover: BytesMut::new(),
            inner_finished: false,
        }
    }
}

impl<B> hyper::body::Body for GrpcWebTextDecodeBody<B>
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

            if *this.inner_finished {
                return match decode_grpc_web_text_final(this.carryover) {
                    Ok(Some(data)) => std::task::Poll::Ready(Some(Ok(Frame::data(data)))),
                    Ok(None) => std::task::Poll::Ready(None),
                    Err(error) => std::task::Poll::Ready(Some(Err(error))),
                };
            }

            match this.inner.as_mut().poll_frame(cx) {
                std::task::Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
                    Ok(data) => match decode_grpc_web_text_chunk(this.carryover, &data) {
                        Ok(Some(decoded)) => {
                            return std::task::Poll::Ready(Some(Ok(Frame::data(decoded))));
                        }
                        Ok(None) => continue,
                        Err(error) => return std::task::Poll::Ready(Some(Err(error))),
                    },
                    Err(frame) => match frame.into_trailers() {
                        Ok(trailers) => match decode_grpc_web_text_final(this.carryover) {
                            Ok(Some(decoded)) => {
                                *this.pending_trailers = Some(trailers);
                                return std::task::Poll::Ready(Some(Ok(Frame::data(decoded))));
                            }
                            Ok(None) => {
                                return std::task::Poll::Ready(Some(Ok(Frame::trailers(trailers))));
                            }
                            Err(error) => return std::task::Poll::Ready(Some(Err(error))),
                        },
                        Err(_) => {
                            return std::task::Poll::Ready(Some(Err(invalid_grpc_web_body(
                                "unexpected downstream request frame for grpc-web-text decoding",
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
                    continue;
                }
                std::task::Poll::Pending => return std::task::Poll::Pending,
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner_finished
            && self.pending_data.is_none()
            && self.pending_trailers.is_none()
            && self.carryover.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}
