use super::super::codec::encode_grpc_web_trailers;
use super::super::*;

pin_project! {
    pub(crate) struct GrpcWebResponseBody<B> {
        #[pin]
        inner: B,
        pending_frame: Option<Bytes>,
        fallback_trailers: Option<HeaderMap>,
        inner_finished: bool,
    }
}

impl<B> GrpcWebResponseBody<B> {
    pub(crate) fn new(inner: B, fallback_trailers: Option<HeaderMap>) -> Self {
        Self { inner, pending_frame: None, fallback_trailers, inner_finished: false }
    }
}

impl<B> hyper::body::Body for GrpcWebResponseBody<B>
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
            if let Some(frame) = this.pending_frame.take() {
                return std::task::Poll::Ready(Some(Ok(Frame::data(frame))));
            }

            if *this.inner_finished {
                return std::task::Poll::Ready(None);
            }

            match this.inner.as_mut().poll_frame(cx) {
                std::task::Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
                    Ok(data) => return std::task::Poll::Ready(Some(Ok(Frame::data(data)))),
                    Err(frame) => match frame.into_trailers() {
                        Ok(trailers) => {
                            this.fallback_trailers.take();
                            *this.pending_frame = Some(encode_grpc_web_trailers(&trailers));
                        }
                        Err(_) => {
                            return std::task::Poll::Ready(Some(Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "unexpected upstream response frame for grpc-web translation",
                            )
                            .into())));
                        }
                    },
                },
                std::task::Poll::Ready(Some(Err(error))) => {
                    *this.inner_finished = true;
                    return std::task::Poll::Ready(Some(Err(error.into())));
                }
                std::task::Poll::Ready(None) => {
                    *this.inner_finished = true;
                    if let Some(trailers) = this.fallback_trailers.take()
                        && !trailers.is_empty()
                    {
                        *this.pending_frame = Some(encode_grpc_web_trailers(&trailers));
                        continue;
                    }
                    return std::task::Poll::Ready(None);
                }
                std::task::Poll::Pending => return std::task::Poll::Pending,
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner_finished && self.pending_frame.is_none()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}
