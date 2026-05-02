use super::super::super::fill::CacheFillReadState;
use super::*;
use crate::handler::{BoxError, full_body};
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::{Frame, SizeHint};

pub(super) fn spawn_streaming_origin_fill(
    mut inner: HttpBody,
    writer: StreamingCacheWriter,
    fill_state: Option<Arc<CacheFillReadState>>,
) {
    let Some(handle) = tokio::runtime::Handle::try_current().ok() else {
        if let Some(fill_state) = fill_state.as_ref() {
            fill_state.fail("streaming cache fill requires an active Tokio runtime");
        }
        return;
    };

    handle.spawn(async move {
        drive_streaming_origin_fill(&mut inner, writer, fill_state).await;
    });
}

async fn drive_streaming_origin_fill(
    inner: &mut HttpBody,
    writer: StreamingCacheWriter,
    fill_state: Option<Arc<CacheFillReadState>>,
) {
    let mut writer = Some(writer);

    while let Some(frame) = inner.frame().await {
        let Ok(frame) = frame else {
            if let Some(fill_state) = fill_state.as_ref() {
                fill_state.fail("upstream body read failed while filling cache");
            }
            return;
        };

        let stream_completed = frame.is_trailers() || inner.is_end_stream();
        let trailers = frame.trailers_ref().cloned();

        if let Some(data) = frame.data_ref() {
            if data.is_empty() {
                if !stream_completed {
                    continue;
                }
            } else {
                let Some(cache_writer) = writer.as_ref() else {
                    return;
                };
                if !cache_writer.send_data(data.clone()).await {
                    if let Some(fill_state) = fill_state.as_ref() {
                        fill_state.fail("streaming cache writer channel closed before EOF");
                    }
                    return;
                }
            }
        }

        if stream_completed {
            let Some(cache_writer) = writer.take() else {
                return;
            };
            if !cache_writer.finish(trailers).await
                && let Some(fill_state) = fill_state.as_ref()
            {
                fill_state.fail("streaming cache writer channel closed before end-of-stream");
            }
            return;
        }
    }

    let Some(cache_writer) = writer.take() else {
        return;
    };
    if !cache_writer.finish(None).await
        && let Some(fill_state) = fill_state.as_ref()
    {
        fill_state.fail("streaming cache writer channel closed before end-of-stream");
    }
}

pub(super) struct StreamingCacheBody {
    inner: HttpBody,
    size_hint: SizeHint,
    cache: Option<StreamingCacheWriter>,
    cached_body_bytes: usize,
    max_entry_bytes: usize,
    done: bool,
}

impl StreamingCacheBody {
    pub(super) fn new(
        inner: HttpBody,
        size_hint: SizeHint,
        cache: StreamingCacheWriter,
        max_entry_bytes: usize,
    ) -> Self {
        Self {
            inner,
            size_hint,
            cache: Some(cache),
            cached_body_bytes: 0,
            max_entry_bytes,
            done: false,
        }
    }

    fn disable_cache(&mut self) {
        self.cache.take();
    }

    fn finish_cache(&mut self, trailers: Option<http::HeaderMap>) {
        let Some(cache) = self.cache.take() else {
            return;
        };
        let _ = cache.try_finish(trailers);
    }

    fn cache_frame_data(&mut self, data: &Bytes) {
        if data.is_empty() {
            return;
        }

        let Some(cache) = self.cache.as_ref() else {
            return;
        };
        let next_body_size = self.cached_body_bytes.saturating_add(data.len());
        if next_body_size > self.max_entry_bytes || !cache.try_send_data(data.clone()) {
            self.disable_cache();
            return;
        }
        self.cached_body_bytes = next_body_size;
    }
}

impl Drop for StreamingCacheBody {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let Some(cache) = self.cache.take() else {
            return;
        };
        let inner = std::mem::replace(&mut self.inner, full_body(Bytes::new()));
        let cached_body_bytes = self.cached_body_bytes;
        let max_entry_bytes = self.max_entry_bytes;
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                drain_remaining_frames(inner, cache, cached_body_bytes, max_entry_bytes).await;
            });
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
        if this.done {
            return std::task::Poll::Ready(None);
        }

        match std::pin::Pin::new(&mut this.inner).poll_frame(cx) {
            std::task::Poll::Ready(Some(Ok(frame))) => {
                let stream_completed = frame.is_trailers() || this.inner.is_end_stream();
                let trailers = frame.trailers_ref().cloned();
                if let Some(data) = frame.data_ref() {
                    this.cache_frame_data(data);
                }
                if stream_completed {
                    this.done = true;
                    this.finish_cache(trailers);
                }
                std::task::Poll::Ready(Some(Ok(frame)))
            }
            std::task::Poll::Ready(Some(Err(error))) => {
                this.done = true;
                this.disable_cache();
                std::task::Poll::Ready(Some(Err(error)))
            }
            std::task::Poll::Ready(None) => {
                this.done = true;
                this.finish_cache(None);
                std::task::Poll::Ready(None)
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.done
    }

    fn size_hint(&self) -> SizeHint {
        self.size_hint.clone()
    }
}

async fn drain_remaining_frames(
    mut inner: HttpBody,
    cache: StreamingCacheWriter,
    mut cached_body_bytes: usize,
    max_entry_bytes: usize,
) {
    while let Some(frame) = inner.frame().await {
        let Ok(frame) = frame else {
            return;
        };
        match frame.into_data() {
            Ok(data) => {
                if data.is_empty() {
                    continue;
                }
                let next_body_size = cached_body_bytes.saturating_add(data.len());
                if next_body_size > max_entry_bytes || !cache.send_data(data).await {
                    return;
                }
                cached_body_bytes = next_body_size;
            }
            Err(frame) => match frame.into_trailers() {
                Ok(trailers) => {
                    let _ = cache.finish(Some(trailers)).await;
                    return;
                }
                Err(_) => continue,
            },
        }
    }
    let _ = cache.finish(None).await;
}
