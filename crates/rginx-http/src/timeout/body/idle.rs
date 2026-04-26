use super::*;

pin_project! {
    #[derive(Debug)]
    pub struct IdleTimeoutBody<B> {
        #[pin]
        inner: B,
        timeout: Duration,
        label: String,
        sleep: Option<Pin<Box<Sleep>>>,
        done: bool,
    }
}

impl<B> IdleTimeoutBody<B> {
    pub fn new(inner: B, timeout: Duration, label: impl Into<String>) -> Self {
        Self { inner, timeout, label: label.into(), sleep: None, done: false }
    }
}

impl<B> Body for IdleTimeoutBody<B>
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
                reset_idle_timer(this.sleep, *this.timeout);
                std::task::Poll::Ready(Some(Ok(frame)))
            }
            std::task::Poll::Ready(Some(Err(error))) => {
                *this.done = true;
                std::task::Poll::Ready(Some(Err(error.into())))
            }
            std::task::Poll::Ready(None) => {
                *this.done = true;
                std::task::Poll::Ready(None)
            }
            std::task::Poll::Pending => {
                match poll_idle_timer(cx, this.sleep, *this.timeout, this.label) {
                    std::task::Poll::Ready(error) => {
                        *this.done = true;
                        std::task::Poll::Ready(Some(Err(error)))
                    }
                    std::task::Poll::Pending => std::task::Poll::Pending,
                }
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.done
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
