use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::time::Sleep;

use crate::handler::BoxError;

pub(super) fn reset_idle_timer(sleep: &mut Option<Pin<Box<Sleep>>>, timeout: Duration) {
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

pub(super) fn poll_idle_timer(
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

pub(super) fn poll_write_side<T>(
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
