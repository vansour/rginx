use std::future::{Future, poll_fn};
use std::io;
use std::io::IoSlice;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http::{HeaderMap, HeaderValue};
use http_body_util::StreamBody;
use hyper::body::{Body, Frame, SizeHint};
use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::{Instant, Sleep};

use super::{GrpcDeadlineBody, IdleTimeoutBody, MaxBytesBody, WriteTimeoutIo};

mod grpc_deadline;
mod idle;
mod max_bytes;
mod write_timeout;

pin_project! {
    struct DelayedFrameBody {
        #[pin]
        delay: Sleep,
        emitted: bool,
    }
}

impl DelayedFrameBody {
    fn new(delay: Duration) -> Self {
        Self { delay: tokio::time::sleep(delay), emitted: false }
    }
}

impl Body for DelayedFrameBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();

        if *this.emitted {
            return Poll::Ready(None);
        }

        match this.delay.as_mut().poll(cx) {
            Poll::Ready(()) => {
                *this.emitted = true;
                Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(b"ok")))))
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.emitted
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

struct EarlyEndTrailersBody {
    state: u8,
}

impl EarlyEndTrailersBody {
    fn new() -> Self {
        Self { state: 0 }
    }
}

impl Body for EarlyEndTrailersBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();

        match this.state {
            0 => {
                this.state = 1;
                Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(b"data")))))
            }
            1 => {
                this.state = 2;
                let mut trailers = http::HeaderMap::new();
                trailers.insert("x-trailer", http::HeaderValue::from_static("present"));
                Poll::Ready(Some(Ok(Frame::trailers(trailers))))
            }
            _ => Poll::Ready(None),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.state >= 1
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

pin_project! {
    struct TwoStageBody {
        #[pin]
        first_delay: Sleep,
        #[pin]
        second_delay: Sleep,
        state: u8,
    }
}

impl TwoStageBody {
    fn new(first_delay: Duration, second_delay: Duration) -> Self {
        Self {
            first_delay: tokio::time::sleep(first_delay),
            second_delay: tokio::time::sleep(second_delay),
            state: 0,
        }
    }
}

impl Body for TwoStageBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();

        match *this.state {
            0 => match this.first_delay.as_mut().poll(cx) {
                Poll::Ready(()) => {
                    *this.state = 1;
                    Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(b"ok")))))
                }
                Poll::Pending => Poll::Pending,
            },
            1 => match this.second_delay.as_mut().poll(cx) {
                Poll::Ready(()) => {
                    *this.state = 2;
                    let mut trailers = HeaderMap::new();
                    trailers.insert("grpc-status", HeaderValue::from_static("0"));
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

pin_project! {
    struct DelayedWriter {
        #[pin]
        delay: Sleep,
        emitted: bool,
    }
}

impl DelayedWriter {
    fn new(delay: Duration) -> Self {
        Self { delay: tokio::time::sleep(delay), emitted: false }
    }
}

impl AsyncRead for DelayedWriter {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for DelayedWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut this = self.project();

        if *this.emitted {
            return Poll::Ready(Ok(buf.len()));
        }

        match this.delay.as_mut().poll(cx) {
            Poll::Ready(()) => {
                *this.emitted = true;
                Poll::Ready(Ok(buf.len()))
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let total = bufs.iter().map(|buf| buf.len()).sum();
        self.poll_write(cx, &vec![0u8; total])
    }

    fn is_write_vectored(&self) -> bool {
        true
    }
}
