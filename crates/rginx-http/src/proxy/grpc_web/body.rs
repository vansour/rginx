use super::codec::{
    decode_grpc_web_request_frame, decode_grpc_web_text_chunk, decode_grpc_web_text_final,
    encode_grpc_web_text_chunk, encode_grpc_web_trailers, flush_grpc_web_text_chunk,
    invalid_grpc_web_body,
};
use super::*;

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
                Ok(Some(super::codec::ParsedGrpcWebRequestFrame::Data(data))) => {
                    return std::task::Poll::Ready(Some(Ok(Frame::data(data))));
                }
                Ok(Some(super::codec::ParsedGrpcWebRequestFrame::Trailers(mut trailers))) => {
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
