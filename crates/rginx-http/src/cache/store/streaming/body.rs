use bytes::Bytes;
use hyper::body::{Frame, SizeHint};
use tokio::io::AsyncWrite;

use super::finalize::{
    abandon_streaming_cache, record_streaming_cache_write_error, start_streaming_cache_finalize,
};
use super::*;
use crate::handler::BoxError;

pub(super) struct StreamingCacheBody {
    inner: HttpBody,
    size_hint: SizeHint,
    expected_body_bytes: Option<usize>,
    pending_frame: Option<Frame<Bytes>>,
    cache: Option<ActiveStreamingCache>,
    finalizing: Option<StreamingCacheFinalize>,
    done: bool,
}

impl StreamingCacheBody {
    pub(super) fn new(inner: HttpBody, size_hint: SizeHint, cache: ActiveStreamingCache) -> Self {
        Self {
            inner,
            expected_body_bytes: size_hint.exact().and_then(|exact| usize::try_from(exact).ok()),
            size_hint,
            pending_frame: None,
            cache: Some(cache),
            finalizing: None,
            done: false,
        }
    }
}

impl Drop for StreamingCacheBody {
    fn drop(&mut self) {
        if let Some(finalizing) = self.finalizing.take() {
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(finalizing);
            }
            return;
        }
        if let Some(cache) = self.cache.take() {
            abandon_streaming_cache(cache);
        }
    }
}

impl hyper::body::Body for StreamingCacheBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();

        loop {
            if let Some(finalizing) = this.finalizing.as_mut() {
                match finalizing.as_mut().poll(cx) {
                    std::task::Poll::Ready(()) => {
                        this.finalizing = None;
                        if let Some(frame) = this.pending_frame.take() {
                            return std::task::Poll::Ready(Some(Ok(frame)));
                        }
                        return std::task::Poll::Ready(None);
                    }
                    std::task::Poll::Pending => return std::task::Poll::Pending,
                }
            }

            let mut should_return_pending_frame = false;
            let mut disable_cache: Option<(std::io::Error, bool)> = None;
            if let Some(cache) = this.cache.as_mut()
                && let Some(pending_write) = cache.pending_write.as_mut()
            {
                match std::pin::Pin::new(&mut cache.file).poll_write(cx, pending_write.remaining())
                {
                    std::task::Poll::Ready(Ok(0)) => {
                        disable_cache = Some((
                            std::io::Error::new(
                                std::io::ErrorKind::WriteZero,
                                "cache body write returned zero bytes",
                            ),
                            true,
                        ));
                    }
                    std::task::Poll::Ready(Ok(written)) => {
                        pending_write.written += written;
                        if pending_write.written == pending_write.bytes.len() {
                            cache.plan.body_size_bytes = cache
                                .plan
                                .body_size_bytes
                                .saturating_add(pending_write.bytes.len());
                            cache.pending_write = None;
                            if cache.finalize_after_pending_frame {
                                cache.finalize_after_pending_frame = false;
                                if let Some(cache) = this.cache.take() {
                                    this.finalizing = Some(start_streaming_cache_finalize(cache));
                                    continue;
                                }
                            } else {
                                should_return_pending_frame = true;
                            }
                        }
                    }
                    std::task::Poll::Ready(Err(error)) => disable_cache = Some((error, true)),
                    std::task::Poll::Pending => return std::task::Poll::Pending,
                }
            }

            if let Some((error, record_write_error)) = disable_cache.take() {
                let frame =
                    this.pending_frame.take().expect("pending frame should accompany writes");
                if let Some(cache) = this.cache.take() {
                    if record_write_error {
                        record_streaming_cache_write_error(&cache.plan, &error);
                    }
                    abandon_streaming_cache(cache);
                }
                return std::task::Poll::Ready(Some(Ok(frame)));
            }
            if should_return_pending_frame {
                let frame = this.pending_frame.take().expect("pending frame should be available");
                return std::task::Poll::Ready(Some(Ok(frame)));
            }
            if this.done {
                return std::task::Poll::Ready(None);
            }

            match std::pin::Pin::new(&mut this.inner).poll_frame(cx) {
                std::task::Poll::Ready(Some(Ok(frame))) => {
                    if let Some(data) = frame.data_ref() {
                        let data = data.clone();
                        let stream_completed = frame.is_trailers()
                            || this.inner.is_end_stream()
                            || this.expected_body_bytes.is_some_and(|expected| {
                                expected
                                    <= this.cache.as_ref().map_or(data.len(), |cache| {
                                        cache.plan.body_size_bytes.saturating_add(data.len())
                                    })
                            });
                        this.done = stream_completed;
                        let mut overflowed = false;
                        if let Some(cache) = this.cache.as_mut() {
                            let body_len = cache.plan.body_size_bytes.saturating_add(data.len());
                            if body_len > cache.plan.zone.config.max_entry_bytes {
                                overflowed = true;
                            } else if !data.is_empty() {
                                this.pending_frame = Some(frame);
                                cache.pending_write = Some(PendingCacheWrite::new(data));
                                cache.finalize_after_pending_frame = stream_completed;
                                continue;
                            } else if stream_completed {
                                this.pending_frame = Some(frame);
                                if let Some(cache) = this.cache.take() {
                                    this.finalizing = Some(start_streaming_cache_finalize(cache));
                                    continue;
                                }
                                let frame = this
                                    .pending_frame
                                    .take()
                                    .expect("terminal empty frame should be available");
                                return std::task::Poll::Ready(Some(Ok(frame)));
                            }
                        }
                        if overflowed && let Some(cache) = this.cache.take() {
                            abandon_streaming_cache(cache);
                        }
                        return std::task::Poll::Ready(Some(Ok(frame)));
                    }

                    let stream_completed = frame.is_trailers() || this.inner.is_end_stream();
                    this.done = stream_completed;
                    if stream_completed && let Some(cache) = this.cache.take() {
                        this.pending_frame = Some(frame);
                        this.finalizing = Some(start_streaming_cache_finalize(cache));
                        continue;
                    }
                    return std::task::Poll::Ready(Some(Ok(frame)));
                }
                std::task::Poll::Ready(Some(Err(error))) => {
                    this.done = true;
                    if let Some(cache) = this.cache.take() {
                        cache.plan.zone.record_write_error();
                        abandon_streaming_cache(cache);
                    }
                    return std::task::Poll::Ready(Some(Err(error)));
                }
                std::task::Poll::Ready(None) => {
                    this.done = true;
                    if let Some(cache) = this.cache.take() {
                        this.finalizing = Some(start_streaming_cache_finalize(cache));
                        continue;
                    }
                    return std::task::Poll::Ready(None);
                }
                std::task::Poll::Pending => return std::task::Poll::Pending,
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.pending_frame.is_none()
            && self.cache.is_none()
            && self.finalizing.is_none()
            && (self.done || self.inner.is_end_stream())
    }

    fn size_hint(&self) -> SizeHint {
        self.size_hint.clone()
    }
}
