use super::*;

#[derive(Debug)]
struct RelayedRequestBody {
    receiver: tokio::sync::mpsc::Receiver<Result<Frame<Bytes>, BoxError>>,
    done: bool,
}

impl RelayedRequestBody {
    fn new(receiver: tokio::sync::mpsc::Receiver<Result<Frame<Bytes>, BoxError>>) -> Self {
        Self { receiver, done: false }
    }
}

impl hyper::body::Body for RelayedRequestBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if self.done {
            return std::task::Poll::Ready(None);
        }

        match self.receiver.poll_recv(cx) {
            std::task::Poll::Ready(Some(Ok(frame))) => {
                if frame.is_trailers() {
                    self.done = true;
                }
                std::task::Poll::Ready(Some(Ok(frame)))
            }
            std::task::Poll::Ready(Some(Err(error))) => {
                self.done = true;
                std::task::Poll::Ready(Some(Err(error)))
            }
            std::task::Poll::Ready(None) => {
                self.done = true;
                std::task::Poll::Ready(None)
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

pub(super) fn relay_streaming_request_body(body: HttpBody) -> (HttpBody, StreamingBodyCompletion) {
    let (frame_tx, frame_rx) = tokio::sync::mpsc::channel(1);
    let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        let mut body = body;
        let result = loop {
            match body.frame().await {
                Some(Ok(frame)) => {
                    let reached_end = frame.is_trailers();
                    let _ = frame_tx.send(Ok(frame)).await;
                    if reached_end {
                        break Ok(());
                    }
                }
                Some(Err(error)) => {
                    let _ = frame_tx.send(Err(clone_box_error(error.as_ref()))).await;
                    break Err(clone_box_error(error.as_ref()));
                }
                None => break Ok(()),
            }
        };

        drop(frame_tx);
        let _ = completion_tx.send(result);
    });

    (boxed_body(RelayedRequestBody::new(frame_rx)), completion_rx)
}

fn clone_box_error(error: &(dyn StdError + 'static)) -> BoxError {
    if let Some(max_request_body_bytes) = request_body_limit_error(error) {
        return Box::new(crate::timeout::RequestBodyLimitError::new(max_request_body_bytes));
    }

    if let Some(io_error) = error.downcast_ref::<std::io::Error>() {
        return Box::new(std::io::Error::new(io_error.kind(), io_error.to_string()));
    }

    Box::new(std::io::Error::other(error.to_string()))
}
