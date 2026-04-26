use super::*;

pub(crate) struct GrpcResponseBody {
    state: u8,
}

impl GrpcResponseBody {
    pub(crate) fn new() -> Self {
        Self { state: 0 }
    }
}

impl Body for GrpcResponseBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();

        match this.state {
            0 => {
                this.state = 1;
                Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(GRPC_RESPONSE_FRAME)))))
            }
            1 => {
                this.state = 2;
                let mut trailers = HeaderMap::new();
                trailers.insert("grpc-status", HeaderValue::from_static("0"));
                trailers.insert("grpc-message", HeaderValue::from_static("ok"));
                Poll::Ready(Some(Ok(Frame::trailers(trailers))))
            }
            _ => Poll::Ready(None),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.state >= 2
    }

    fn size_hint(&self) -> SizeHint {
        let mut hint = SizeHint::new();
        hint.set_exact(GRPC_RESPONSE_FRAME.len() as u64);
        hint
    }
}

#[derive(Clone, Copy)]
pub(crate) enum UpstreamResponseMode {
    Immediate,
    DelayHeaders(Duration),
    DelayBody(Duration),
}

pub(crate) struct DelayedGrpcResponseBody {
    state: u8,
    delay: Pin<Box<tokio::time::Sleep>>,
}

impl DelayedGrpcResponseBody {
    pub(crate) fn new(delay: Duration) -> Self {
        Self { state: 0, delay: Box::pin(tokio::time::sleep(delay)) }
    }
}

pub(crate) enum EitherGrpcResponseBody {
    Immediate(GrpcResponseBody),
    Delayed(DelayedGrpcResponseBody),
    Cancellable(CancellableGrpcResponseBody),
    Full(Full<Bytes>),
}

impl Body for DelayedGrpcResponseBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();

        match this.state {
            0 => match this.delay.as_mut().poll(cx) {
                Poll::Ready(()) => {
                    this.state = 1;
                    Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(GRPC_RESPONSE_FRAME)))))
                }
                Poll::Pending => Poll::Pending,
            },
            1 => {
                this.state = 2;
                let mut trailers = HeaderMap::new();
                trailers.insert("grpc-status", HeaderValue::from_static("0"));
                trailers.insert("grpc-message", HeaderValue::from_static("ok"));
                Poll::Ready(Some(Ok(Frame::trailers(trailers))))
            }
            _ => Poll::Ready(None),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.state >= 2
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

impl Body for EitherGrpcResponseBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match self.get_mut() {
            Self::Immediate(body) => Pin::new(body).poll_frame(cx),
            Self::Delayed(body) => Pin::new(body).poll_frame(cx),
            Self::Cancellable(body) => Pin::new(body).poll_frame(cx),
            Self::Full(body) => Pin::new(body).poll_frame(cx),
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            Self::Immediate(body) => body.is_end_stream(),
            Self::Delayed(body) => body.is_end_stream(),
            Self::Cancellable(body) => body.is_end_stream(),
            Self::Full(body) => body.is_end_stream(),
        }
    }

    fn size_hint(&self) -> SizeHint {
        match self {
            Self::Immediate(body) => body.size_hint(),
            Self::Delayed(body) => body.size_hint(),
            Self::Cancellable(body) => body.size_hint(),
            Self::Full(body) => body.size_hint(),
        }
    }
}

pub(crate) struct CancellableGrpcResponseBody {
    state: u8,
    delay: Pin<Box<tokio::time::Sleep>>,
    cancelled_tx: Option<Arc<Mutex<Option<oneshot::Sender<()>>>>>,
}

impl CancellableGrpcResponseBody {
    pub(crate) fn new(cancelled_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>) -> Self {
        Self {
            state: 0,
            delay: Box::pin(tokio::time::sleep(Duration::from_secs(30))),
            cancelled_tx: Some(cancelled_tx),
        }
    }
}

impl Drop for CancellableGrpcResponseBody {
    fn drop(&mut self) {
        if let Some(cancelled_tx) = self.cancelled_tx.take()
            && let Some(sender) =
                cancelled_tx.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take()
        {
            let _ = sender.send(());
        }
    }
}

impl Body for CancellableGrpcResponseBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();

        match this.state {
            0 => {
                this.state = 1;
                Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(GRPC_RESPONSE_FRAME)))))
            }
            1 => match this.delay.as_mut().poll(cx) {
                Poll::Ready(()) => {
                    this.state = 2;
                    let mut trailers = HeaderMap::new();
                    trailers.insert("grpc-status", HeaderValue::from_static("0"));
                    trailers.insert("grpc-message", HeaderValue::from_static("ok"));
                    Poll::Ready(Some(Ok(Frame::trailers(trailers))))
                }
                Poll::Pending => Poll::Pending,
            },
            _ => Poll::Ready(None),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.state >= 2
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

pub(crate) struct GrpcRequestBody {
    state: u8,
}

impl GrpcRequestBody {
    pub(crate) fn new() -> Self {
        Self { state: 0 }
    }
}

impl Body for GrpcRequestBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();

        match this.state {
            0 => {
                this.state = 1;
                Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(GRPC_REQUEST_FRAME)))))
            }
            1 => {
                this.state = 2;
                let mut trailers = HeaderMap::new();
                trailers.insert("x-client-trailer", HeaderValue::from_static("sent"));
                trailers.insert("x-request-checksum", HeaderValue::from_static("abc123"));
                Poll::Ready(Some(Ok(Frame::trailers(trailers))))
            }
            _ => Poll::Ready(None),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.state >= 2
    }

    fn size_hint(&self) -> SizeHint {
        let mut hint = SizeHint::new();
        hint.set_exact(GRPC_REQUEST_FRAME.len() as u64);
        hint
    }
}
