use bytes::{Buf, Bytes};
use http::HeaderMap;
use hyper::body::{Frame, SizeHint};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::handler::{BoxError, HttpBody, boxed_body};

use super::session::{H3RequestStream, Http3Session};

struct StreamingResponseBody {
    rx: mpsc::Receiver<Result<Frame<Bytes>, BoxError>>,
    size_hint: SizeHint,
    done: bool,
    join_handle: Option<JoinHandle<()>>,
}

impl StreamingResponseBody {
    fn new(
        rx: mpsc::Receiver<Result<Frame<Bytes>, BoxError>>,
        size_hint: SizeHint,
        join_handle: JoinHandle<()>,
    ) -> Self {
        Self { rx, size_hint, done: false, join_handle: Some(join_handle) }
    }
}

impl Drop for StreamingResponseBody {
    fn drop(&mut self) {
        if let Some(join_handle) = self.join_handle.take()
            && !join_handle.is_finished()
        {
            join_handle.abort();
        }
    }
}

impl hyper::body::Body for StreamingResponseBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.as_mut().get_mut();
        match this.rx.poll_recv(cx) {
            std::task::Poll::Ready(None) => {
                this.done = true;
                std::task::Poll::Ready(None)
            }
            other => other,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.done
    }

    fn size_hint(&self) -> SizeHint {
        self.size_hint.clone()
    }
}

pub(super) fn streaming_response_body(
    mut request_stream: H3RequestStream,
    session: std::sync::Arc<Http3Session>,
    peer_url: String,
    size_hint: SizeHint,
) -> HttpBody {
    let (tx, rx) = mpsc::channel(1);
    let join_handle = tokio::spawn(async move {
        loop {
            let next = match request_stream.recv_data().await {
                Ok(chunk) => chunk,
                Err(error) if is_clean_http3_response_shutdown(&error) => None,
                Err(error) => {
                    session.mark_closed();
                    let _ = tx
                        .send(Err::<Frame<Bytes>, BoxError>(
                            std::io::Error::other(format!(
                                "failed to receive upstream http3 response body from `{peer_url}`: {error}"
                            ))
                            .into(),
                        ))
                        .await;
                    return;
                }
            };
            let Some(mut chunk) = next else {
                break;
            };
            let bytes = chunk.copy_to_bytes(chunk.remaining());
            if tx.send(Ok(Frame::data(bytes))).await.is_err() {
                return;
            }
        }

        match request_stream.recv_trailers().await {
            Ok(Some(trailers)) => {
                let _ = tx.send(Ok(Frame::trailers(trailers))).await;
            }
            Ok(None) => {}
            Err(error) if is_clean_http3_response_shutdown(&error) => {}
            Err(error) => {
                session.mark_closed();
                let _ = tx
                    .send(Err::<Frame<Bytes>, BoxError>(
                        std::io::Error::other(format!(
                            "failed to receive upstream http3 response trailers from `{peer_url}`: {error}"
                        ))
                        .into(),
                    ))
                    .await;
            }
        }
    });

    boxed_body(StreamingResponseBody::new(rx, size_hint, join_handle))
}

pub(super) fn response_size_hint(headers: &HeaderMap) -> SizeHint {
    let mut hint = SizeHint::default();
    if let Some(content_length) = headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
    {
        hint.set_exact(content_length);
    }
    hint
}

fn is_clean_http3_response_shutdown(error: &impl std::fmt::Display) -> bool {
    let error = error.to_string();
    error.contains("ApplicationClose: H3_NO_ERROR")
        || error.contains("Application { code: H3_NO_ERROR")
}
