use super::*;
use pin_project_lite::pin_project;

pin_project! {
    pub(super) struct Http3RequestBody<S> {
        #[pin]
        stream: RequestStream<S, Bytes>,
        state: crate::state::SharedState,
        listener_id: String,
        data_finished: bool,
        trailers_finished: bool,
        error_recorded: bool,
    }
}

impl<S> Http3RequestBody<S> {
    pub(super) fn new(
        stream: RequestStream<S, Bytes>,
        state: crate::state::SharedState,
        listener_id: String,
    ) -> Self {
        Self {
            stream,
            state,
            listener_id,
            data_finished: false,
            trailers_finished: false,
            error_recorded: false,
        }
    }
}

impl<S> Body for Http3RequestBody<S>
where
    S: RecvStream,
{
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<std::result::Result<Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();

        if !*this.data_finished {
            match this.stream.as_mut().poll_recv_data(cx) {
                Poll::Ready(Ok(Some(mut chunk))) => {
                    let bytes = chunk.copy_to_bytes(chunk.remaining());
                    return Poll::Ready(Some(Ok(Frame::data(bytes))));
                }
                Poll::Ready(Ok(None)) => {
                    *this.data_finished = true;
                }
                Poll::Ready(Err(error)) => {
                    if !*this.error_recorded {
                        this.state
                            .record_http3_request_body_stream_error(this.listener_id.as_str());
                        *this.error_recorded = true;
                    }
                    *this.data_finished = true;
                    *this.trailers_finished = true;
                    return Poll::Ready(Some(Err(stream_error(error))));
                }
                Poll::Pending => return Poll::Pending,
            }
        }

        if !*this.trailers_finished {
            match this.stream.as_mut().poll_recv_trailers(cx) {
                Poll::Ready(Ok(Some(trailers))) => {
                    *this.trailers_finished = true;
                    return Poll::Ready(Some(Ok(Frame::trailers(trailers))));
                }
                Poll::Ready(Ok(None)) => {
                    *this.trailers_finished = true;
                }
                Poll::Ready(Err(error)) => {
                    if !*this.error_recorded {
                        this.state
                            .record_http3_request_body_stream_error(this.listener_id.as_str());
                        *this.error_recorded = true;
                    }
                    *this.trailers_finished = true;
                    return Poll::Ready(Some(Err(stream_error(error))));
                }
                Poll::Pending => return Poll::Pending,
            }
        }

        Poll::Ready(None)
    }

    fn is_end_stream(&self) -> bool {
        self.data_finished && self.trailers_finished
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

pub(super) fn stream_error(error: impl std::fmt::Display) -> BoxError {
    std::io::Error::other(format!("{error}")).into()
}
