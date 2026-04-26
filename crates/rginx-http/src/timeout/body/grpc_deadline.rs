use super::*;

pin_project! {
    #[derive(Debug)]
    pub struct GrpcDeadlineBody<B> {
        #[pin]
        inner: B,
        deadline: Pin<Box<Sleep>>,
        timeout: Duration,
        label: String,
        timeout_message: String,
        done: bool,
    }
}

impl<B> GrpcDeadlineBody<B> {
    pub fn new(
        inner: B,
        deadline: Instant,
        timeout: Duration,
        label: impl Into<String>,
        timeout_message: impl Into<String>,
    ) -> Self {
        Self {
            inner,
            deadline: Box::pin(tokio::time::sleep_until(deadline)),
            timeout,
            label: label.into(),
            timeout_message: timeout_message.into(),
            done: false,
        }
    }
}

impl<B> Body for GrpcDeadlineBody<B>
where
    B: Body<Data = Bytes>,
    B::Error: Into<BoxError>,
{
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();

        if *this.done {
            return std::task::Poll::Ready(None);
        }

        match this.inner.as_mut().poll_frame(cx) {
            std::task::Poll::Ready(Some(Ok(frame))) => {
                if frame.is_trailers() {
                    *this.done = true;
                }
                return std::task::Poll::Ready(Some(Ok(frame)));
            }
            std::task::Poll::Ready(Some(Err(error))) => {
                *this.done = true;
                return std::task::Poll::Ready(Some(Err(error.into())));
            }
            std::task::Poll::Ready(None) => {
                *this.done = true;
                return std::task::Poll::Ready(None);
            }
            std::task::Poll::Pending => {}
        }

        match this.deadline.as_mut().poll(cx) {
            std::task::Poll::Ready(()) => {
                *this.done = true;
                tracing::warn!(
                    timeout_ms = this.timeout.as_millis() as u64,
                    body = %this.label,
                    "gRPC response deadline reached"
                );
                std::task::Poll::Ready(Some(Ok(Frame::trailers(grpc_deadline_exceeded_trailers(
                    this.timeout_message.as_str(),
                )))))
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.done
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}
