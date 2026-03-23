use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use hyper::body::{Body, Frame, SizeHint};
use pin_project_lite::pin_project;
use tokio::time::Sleep;

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
        self.done || self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
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

#[cfg(test)]
mod tests {
    use std::future::{Future, poll_fn};
    use std::io;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::time::Duration;

    use bytes::Bytes;
    use hyper::body::{Body, Frame, SizeHint};
    use pin_project_lite::pin_project;
    use tokio::time::Sleep;

    use super::IdleTimeoutBody;

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
}
