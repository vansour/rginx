use std::future::Future;
use std::io;
use std::io::IoSlice;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http::HeaderMap;
use http::header::{HeaderName, HeaderValue};
use hyper::body::{Body, Frame, SizeHint};
use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::{Instant, Sleep};

use crate::handler::BoxError;

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

pin_project! {
    #[derive(Debug)]
    pub struct WriteTimeoutIo<T> {
        #[pin]
        inner: T,
        timeout: Option<Duration>,
        label: String,
        sleep: Option<Pin<Box<Sleep>>>,
    }
}

impl<T> WriteTimeoutIo<T> {
    pub fn new(inner: T, timeout: Option<Duration>, label: impl Into<String>) -> Self {
        Self { inner, timeout, label: label.into(), sleep: None }
    }
}

impl<T> AsyncRead for WriteTimeoutIo<T>
where
    T: AsyncRead,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.project().inner.poll_read(cx, buf)
    }
}

impl<T> AsyncWrite for WriteTimeoutIo<T>
where
    T: AsyncWrite,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut this = self.project();
        let result = this.inner.as_mut().poll_write(cx, buf);
        poll_write_side(cx, result, *this.timeout, this.sleep, this.label.as_str(), "write")
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut this = self.project();
        let result = this.inner.as_mut().poll_flush(cx);
        poll_write_side(cx, result, *this.timeout, this.sleep, this.label.as_str(), "flush")
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut this = self.project();
        let result = this.inner.as_mut().poll_shutdown(cx);
        poll_write_side(cx, result, *this.timeout, this.sleep, this.label.as_str(), "shutdown")
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let mut this = self.project();
        let result = this.inner.as_mut().poll_write_vectored(cx, bufs);
        poll_write_side(cx, result, *this.timeout, this.sleep, this.label.as_str(), "write")
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
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
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();

        if *this.done {
            return Poll::Ready(None);
        }

        match this.inner.as_mut().poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if frame.is_trailers() {
                    *this.done = true;
                }
                reset_idle_timer(this.sleep, *this.timeout);
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(error))) => {
                *this.done = true;
                Poll::Ready(Some(Err(error.into())))
            }
            Poll::Ready(None) => {
                *this.done = true;
                Poll::Ready(None)
            }
            Poll::Pending => match poll_idle_timer(cx, this.sleep, *this.timeout, this.label) {
                Poll::Ready(error) => {
                    *this.done = true;
                    Poll::Ready(Some(Err(error)))
                }
                Poll::Pending => Poll::Pending,
            },
        }
    }

    fn is_end_stream(&self) -> bool {
        self.done
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
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
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();

        if *this.done {
            return Poll::Ready(None);
        }

        match this.inner.as_mut().poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if frame.is_trailers() {
                    *this.done = true;
                }
                return Poll::Ready(Some(Ok(frame)));
            }
            Poll::Ready(Some(Err(error))) => {
                *this.done = true;
                return Poll::Ready(Some(Err(error.into())));
            }
            Poll::Ready(None) => {
                *this.done = true;
                return Poll::Ready(None);
            }
            Poll::Pending => {}
        }

        match this.deadline.as_mut().poll(cx) {
            Poll::Ready(()) => {
                *this.done = true;
                tracing::warn!(
                    timeout_ms = this.timeout.as_millis() as u64,
                    body = %this.label,
                    "gRPC response deadline reached"
                );
                Poll::Ready(Some(Ok(Frame::trailers(grpc_deadline_exceeded_trailers(
                    this.timeout_message.as_str(),
                )))))
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.done
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

fn reset_idle_timer(sleep: &mut Option<Pin<Box<Sleep>>>, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    match sleep {
        Some(sleep) => sleep.as_mut().reset(deadline),
        None => *sleep = Some(Box::pin(tokio::time::sleep_until(deadline))),
    }
}

fn arm_idle_timer(sleep: &mut Option<Pin<Box<Sleep>>>, timeout: Duration) {
    if sleep.is_none() {
        *sleep = Some(Box::pin(tokio::time::sleep_until(tokio::time::Instant::now() + timeout)));
    }
}

fn clear_idle_timer(sleep: &mut Option<Pin<Box<Sleep>>>) {
    *sleep = None;
}

fn grpc_deadline_exceeded_trailers(message: &str) -> HeaderMap {
    let mut trailers = HeaderMap::new();
    trailers.insert(HeaderName::from_static("grpc-status"), HeaderValue::from_static("4"));
    if !message.is_empty() {
        trailers.insert(
            HeaderName::from_static("grpc-message"),
            HeaderValue::from_str(message).expect("gRPC timeout message should be a valid header"),
        );
    }
    trailers
}

fn poll_idle_timer(
    cx: &mut Context<'_>,
    sleep: &mut Option<Pin<Box<Sleep>>>,
    timeout: Duration,
    label: &str,
) -> Poll<BoxError> {
    arm_idle_timer(sleep, timeout);

    match sleep.as_mut().expect("idle timer should be armed").as_mut().poll(cx) {
        Poll::Ready(()) => {
            tracing::warn!(
                timeout_ms = timeout.as_millis() as u64,
                body = %label,
                "streaming body idle timeout reached"
            );
            Poll::Ready(Box::new(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{label} stalled for {} ms", timeout.as_millis()),
            )))
        }
        Poll::Pending => Poll::Pending,
    }
}

fn poll_write_side<T>(
    cx: &mut Context<'_>,
    result: Poll<io::Result<T>>,
    timeout: Option<Duration>,
    sleep: &mut Option<Pin<Box<Sleep>>>,
    label: &str,
    operation: &str,
) -> Poll<io::Result<T>> {
    match result {
        Poll::Ready(result) => {
            clear_idle_timer(sleep);
            Poll::Ready(result)
        }
        Poll::Pending => {
            let Some(timeout) = timeout else {
                return Poll::Pending;
            };

            match poll_write_timeout(cx, sleep, timeout, label, operation) {
                Poll::Ready(error) => Poll::Ready(Err(error)),
                Poll::Pending => Poll::Pending,
            }
        }
    }
}

fn poll_write_timeout(
    cx: &mut Context<'_>,
    sleep: &mut Option<Pin<Box<Sleep>>>,
    timeout: Duration,
    label: &str,
    operation: &str,
) -> Poll<io::Error> {
    arm_idle_timer(sleep, timeout);

    match sleep.as_mut().expect("write timer should be armed").as_mut().poll(cx) {
        Poll::Ready(()) => {
            tracing::warn!(
                timeout_ms = timeout.as_millis() as u64,
                io = %label,
                operation,
                "downstream write timeout reached"
            );
            Poll::Ready(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{label} {operation} stalled for {} ms", timeout.as_millis()),
            ))
        }
        Poll::Pending => Poll::Pending,
    }
}

#[cfg(test)]
mod tests {
    use std::future::{Future, poll_fn};
    use std::io;
    use std::io::IoSlice;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::time::Duration;

    use bytes::Bytes;
    use http::{HeaderMap, HeaderValue};
    use hyper::body::{Body, Frame, SizeHint};
    use pin_project_lite::pin_project;
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tokio::time::{Instant, Sleep};

    use super::{GrpcDeadlineBody, IdleTimeoutBody, WriteTimeoutIo};

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

    #[tokio::test]
    async fn idle_timeout_body_times_out_when_no_frame_arrives() {
        let mut body = Box::pin(IdleTimeoutBody::new(
            DelayedFrameBody::new(Duration::from_millis(60)),
            Duration::from_millis(20),
            "upstream `backend` response body",
        ));

        let error = poll_fn(|cx| body.as_mut().poll_frame(cx))
            .await
            .expect("timeout should resolve as a body error")
            .expect_err("body should time out before the frame arrives");

        assert!(error.to_string().contains("stalled for 20 ms"));
    }

    #[tokio::test]
    async fn idle_timeout_body_allows_frames_that_arrive_in_time() {
        let mut body = Box::pin(IdleTimeoutBody::new(
            DelayedFrameBody::new(Duration::from_millis(10)),
            Duration::from_millis(50),
            "upstream `backend` response body",
        ));

        let frame = poll_fn(|cx| body.as_mut().poll_frame(cx))
            .await
            .expect("body should yield one frame")
            .expect("frame should be successful");
        let bytes = frame.into_data().expect("frame should contain data");

        assert_eq!(bytes, Bytes::from_static(b"ok"));
        assert!(poll_fn(|cx| body.as_mut().poll_frame(cx)).await.is_none());
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

    #[tokio::test]
    async fn idle_timeout_body_waits_for_terminal_trailer_frame() {
        let mut body = Box::pin(IdleTimeoutBody::new(
            EarlyEndTrailersBody::new(),
            Duration::from_secs(1),
            "upstream `backend` response body",
        ));

        assert!(!body.as_ref().get_ref().is_end_stream());

        let first = poll_fn(|cx| body.as_mut().poll_frame(cx))
            .await
            .expect("body should yield a data frame")
            .expect("data frame should be successful");
        assert_eq!(
            first.into_data().expect("first frame should contain data"),
            Bytes::from_static(b"data")
        );
        assert!(!body.as_ref().get_ref().is_end_stream());

        let second = poll_fn(|cx| body.as_mut().poll_frame(cx))
            .await
            .expect("body should yield a trailers frame")
            .expect("trailers frame should be successful");
        let trailers = second.into_trailers().expect("second frame should contain trailers");
        assert_eq!(
            trailers.get("x-trailer").and_then(|value| value.to_str().ok()),
            Some("present")
        );
        assert!(body.as_ref().get_ref().is_end_stream());
        assert!(poll_fn(|cx| body.as_mut().poll_frame(cx)).await.is_none());
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

    #[tokio::test]
    async fn grpc_deadline_body_emits_deadline_exceeded_trailers_before_first_frame() {
        let deadline = Instant::now() + Duration::from_millis(20);
        let mut body = Box::pin(GrpcDeadlineBody::new(
            DelayedFrameBody::new(Duration::from_millis(60)),
            deadline,
            Duration::from_millis(20),
            "upstream `backend` response body",
            "upstream `backend` timed out after 20 ms",
        ));

        let frame = poll_fn(|cx| body.as_mut().poll_frame(cx))
            .await
            .expect("deadline should emit a trailers frame")
            .expect("deadline trailers should be successful");
        let trailers = frame.into_trailers().expect("deadline should surface as trailers");

        assert_eq!(trailers.get("grpc-status").and_then(|value| value.to_str().ok()), Some("4"));
        assert_eq!(
            trailers.get("grpc-message").and_then(|value| value.to_str().ok()),
            Some("upstream `backend` timed out after 20 ms")
        );
        assert!(poll_fn(|cx| body.as_mut().poll_frame(cx)).await.is_none());
    }

    #[tokio::test]
    async fn grpc_deadline_body_keeps_absolute_deadline_after_progress() {
        let deadline = Instant::now() + Duration::from_millis(30);
        let mut body = Box::pin(GrpcDeadlineBody::new(
            TwoStageBody::new(Duration::from_millis(5), Duration::from_millis(80)),
            deadline,
            Duration::from_millis(30),
            "upstream `backend` response body",
            "upstream `backend` timed out after 30 ms",
        ));

        let first = poll_fn(|cx| body.as_mut().poll_frame(cx))
            .await
            .expect("body should yield the first data frame")
            .expect("first frame should be successful");
        assert_eq!(
            first.into_data().expect("first frame should contain data"),
            Bytes::from_static(b"ok")
        );

        let second = poll_fn(|cx| body.as_mut().poll_frame(cx))
            .await
            .expect("deadline should terminate the stream with trailers")
            .expect("deadline trailers should be successful");
        let trailers = second.into_trailers().expect("deadline should surface as trailers");
        assert_eq!(trailers.get("grpc-status").and_then(|value| value.to_str().ok()), Some("4"));
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

    #[tokio::test]
    async fn write_timeout_io_times_out_when_write_stalls() {
        let mut writer = Box::pin(WriteTimeoutIo::new(
            DelayedWriter::new(Duration::from_millis(60)),
            Some(Duration::from_millis(20)),
            "downstream response to 127.0.0.1:8080",
        ));

        let error = poll_fn(|cx| writer.as_mut().poll_write(cx, b"ok"))
            .await
            .expect_err("writer should time out before write readiness");

        assert!(error.to_string().contains("stalled for 20 ms"));
    }

    #[tokio::test]
    async fn write_timeout_io_allows_write_when_progress_arrives_in_time() {
        let mut writer = Box::pin(WriteTimeoutIo::new(
            DelayedWriter::new(Duration::from_millis(10)),
            Some(Duration::from_millis(50)),
            "downstream response to 127.0.0.1:8080",
        ));

        let written = poll_fn(|cx| writer.as_mut().poll_write(cx, b"ok"))
            .await
            .expect("writer should make progress before timing out");

        assert_eq!(written, 2);
    }
}
