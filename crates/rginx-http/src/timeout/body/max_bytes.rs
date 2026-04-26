use super::*;

#[derive(Debug)]
pub struct RequestBodyLimitError {
    max_request_body_bytes: usize,
}

impl RequestBodyLimitError {
    pub fn new(max_request_body_bytes: usize) -> Self {
        Self { max_request_body_bytes }
    }

    pub fn max_request_body_bytes(&self) -> usize {
        self.max_request_body_bytes
    }
}

impl std::fmt::Display for RequestBodyLimitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "request body exceeded configured limit of {} bytes",
            self.max_request_body_bytes
        )
    }
}

impl std::error::Error for RequestBodyLimitError {}

pin_project! {
    #[derive(Debug)]
    pub struct MaxBytesBody<B> {
        #[pin]
        inner: B,
        max_request_body_bytes: usize,
        bytes_read: usize,
        done: bool,
    }
}

impl<B> MaxBytesBody<B> {
    pub fn new(inner: B, max_request_body_bytes: usize) -> Self {
        Self { inner, max_request_body_bytes, bytes_read: 0, done: false }
    }
}

impl<B> Body for MaxBytesBody<B>
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
            std::task::Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
                Ok(data) => {
                    if data.len() > this.max_request_body_bytes.saturating_sub(*this.bytes_read) {
                        *this.done = true;
                        return std::task::Poll::Ready(Some(Err(Box::new(
                            RequestBodyLimitError::new(*this.max_request_body_bytes),
                        ))));
                    }

                    *this.bytes_read += data.len();
                    std::task::Poll::Ready(Some(Ok(Frame::data(data))))
                }
                Err(frame) => match frame.into_trailers() {
                    Ok(trailers) => {
                        *this.done = true;
                        std::task::Poll::Ready(Some(Ok(Frame::trailers(trailers))))
                    }
                    Err(frame) => std::task::Poll::Ready(Some(Ok(frame))),
                },
            },
            std::task::Poll::Ready(Some(Err(error))) => {
                *this.done = true;
                std::task::Poll::Ready(Some(Err(error.into())))
            }
            std::task::Poll::Ready(None) => {
                *this.done = true;
                std::task::Poll::Ready(None)
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.done || self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        let remaining = self.max_request_body_bytes.saturating_sub(self.bytes_read) as u64;
        let inner = self.inner.size_hint();
        let mut hint = SizeHint::new();
        hint.set_lower(inner.lower().min(remaining));
        hint.set_upper(inner.upper().map_or(remaining, |upper| upper.min(remaining)));
        hint
    }
}
