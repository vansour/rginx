use super::super::codec::{
    encode_grpc_web_text_chunk, encode_grpc_web_trailers, flush_grpc_web_text_chunk,
    invalid_grpc_web_body,
};
use super::super::*;

pin_project! {
    pub(crate) struct GrpcWebTextEncodeBody<B> {
        #[pin]
        inner: B,
        pending_data: Option<Bytes>,
        carryover: BytesMut,
        inner_finished: bool,
    }
}

impl<B> GrpcWebTextEncodeBody<B> {
    pub(crate) fn new(inner: B) -> Self {
        Self { inner, pending_data: None, carryover: BytesMut::new(), inner_finished: false }
    }
}

impl<B> hyper::body::Body for GrpcWebTextEncodeBody<B>
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

            if *this.inner_finished {
                if let Some(data) = flush_grpc_web_text_chunk(this.carryover) {
                    return std::task::Poll::Ready(Some(Ok(Frame::data(data))));
                }
                return std::task::Poll::Ready(None);
            }

            match this.inner.as_mut().poll_frame(cx) {
                std::task::Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
                    Ok(data) => {
                        if let Some(encoded) = encode_grpc_web_text_chunk(this.carryover, &data) {
                            return std::task::Poll::Ready(Some(Ok(Frame::data(encoded))));
                        }
                    }
                    Err(frame) => match frame.into_trailers() {
                        Ok(trailers) => {
                            let encoded_trailers = encode_grpc_web_trailers(&trailers);
                            if let Some(encoded) =
                                encode_grpc_web_text_chunk(this.carryover, &encoded_trailers)
                            {
                                return std::task::Poll::Ready(Some(Ok(Frame::data(encoded))));
                            }
                        }
                        Err(_) => {
                            return std::task::Poll::Ready(Some(Err(invalid_grpc_web_body(
                                "unexpected upstream response frame for grpc-web-text encoding",
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
        self.inner_finished && self.pending_data.is_none() && self.carryover.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}
