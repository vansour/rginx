use std::io;
use std::io::IoSlice;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::Sleep;

use super::timers::poll_write_side;

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
