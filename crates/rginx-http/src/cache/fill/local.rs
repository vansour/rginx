use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http::{HeaderMap, StatusCode};
use hyper::body::Frame;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;

use super::super::store::range::build_downstream_response;
use super::super::{CacheRequest, RouteCachePolicy};
use super::common::{
    IN_FLIGHT_FILL_READ_CHUNK_BYTES, inflight_response_parts, size_hint_from_headers,
};
use super::shared::SharedFillExternalStateHandle;
use crate::handler::{BoxError, HttpBody, boxed_body, full_body};

pub(in crate::cache) struct CacheFillReadState {
    status: StatusCode,
    headers: HeaderMap,
    body_tmp_path: PathBuf,
    body_path: PathBuf,
    bytes_written: AtomicU64,
    finished: AtomicBool,
    upstream_completed: AtomicBool,
    notify: Arc<Notify>,
    trailers: Mutex<Option<HeaderMap>>,
    error: Mutex<Option<String>>,
    external_state: Option<SharedFillExternalStateHandle>,
}

impl CacheFillReadState {
    pub(in crate::cache) fn new(
        status: StatusCode,
        headers: HeaderMap,
        body_tmp_path: PathBuf,
        body_path: PathBuf,
        notify: Arc<Notify>,
        external_state: Option<SharedFillExternalStateHandle>,
    ) -> Self {
        let state = Self {
            status,
            headers,
            body_tmp_path,
            body_path,
            bytes_written: AtomicU64::new(0),
            finished: AtomicBool::new(false),
            upstream_completed: AtomicBool::new(false),
            notify,
            trailers: Mutex::new(None),
            error: Mutex::new(None),
            external_state,
        };
        if let Some(external_state) = state.external_state.as_ref()
            && let Err(error) = external_state.publish_response(
                state.status,
                &state.headers,
                &state.body_tmp_path,
                &state.body_path,
            )
        {
            tracing::warn!(%error, "failed to publish shared fill state metadata");
        }
        state
    }

    pub(in crate::cache) fn record_bytes_written(&self, body_size_bytes: usize) {
        self.bytes_written.store(body_size_bytes as u64, Ordering::Release);
        self.notify.notify_waiters();
        if let Some(external_state) = self.external_state.as_ref()
            && let Err(error) = external_state.heartbeat()
        {
            tracing::warn!(%error, "failed to heartbeat shared fill state");
        }
    }

    pub(in crate::cache) fn finish(&self, trailers: Option<HeaderMap>) {
        *self.trailers.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = trailers.clone();
        self.finished.store(true, Ordering::Release);
        self.notify.notify_waiters();
        if let Some(external_state) = self.external_state.as_ref()
            && let Err(error) = external_state.finish(trailers)
        {
            tracing::warn!(%error, "failed to mark shared fill state complete");
        }
    }

    pub(in crate::cache) fn fail(&self, error: impl std::fmt::Display) {
        let error = error.to_string();
        *self.error.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error.clone());
        self.notify.notify_waiters();
        if let Some(external_state) = self.external_state.as_ref()
            && let Err(external_error) = external_state.fail(&error)
        {
            tracing::warn!(%external_error, "failed to mark shared fill state failed");
        }
    }

    pub(in crate::cache) fn can_share(&self) -> bool {
        self.can_serve() && !self.upstream_completed.load(Ordering::Acquire)
    }

    pub(in crate::cache) fn mark_upstream_complete(&self) {
        self.upstream_completed.store(true, Ordering::Release);
        self.notify.notify_waiters();
        if let Some(external_state) = self.external_state.as_ref()
            && let Err(error) = external_state.mark_upstream_complete()
        {
            tracing::warn!(%error, "failed to mark shared fill state upstream-complete");
        }
    }

    fn can_serve(&self) -> bool {
        self.error.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref().is_none()
    }

    fn bytes_written(&self) -> u64 {
        self.bytes_written.load(Ordering::Acquire)
    }

    fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    fn trailers(&self) -> Option<HeaderMap> {
        self.trailers.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    fn error_message(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        size_hint_from_headers(&self.headers)
    }
}

pub(in crate::cache) fn build_inflight_fill_response(
    state: Arc<CacheFillReadState>,
    request: &CacheRequest,
    policy: &RouteCachePolicy,
    read_body: bool,
) -> std::io::Result<crate::handler::HttpResponse> {
    let (parts, trim_plan) =
        inflight_response_parts(state.status, &state.headers, request, policy)?;
    if read_body {
        return Ok(build_downstream_response(parts, InFlightFillBody::new(state), trim_plan));
    }
    Ok(build_downstream_response(parts, full_body(Bytes::new()), trim_plan))
}

pub(in crate::cache) fn inflight_fill_body(state: Arc<CacheFillReadState>) -> HttpBody {
    boxed_body(InFlightFillBody::new(state))
}

struct InFlightFillBody {
    rx: mpsc::Receiver<std::result::Result<Frame<Bytes>, BoxError>>,
    size_hint: hyper::body::SizeHint,
    done: bool,
    join_handle: Option<JoinHandle<()>>,
}

impl InFlightFillBody {
    fn new(state: Arc<CacheFillReadState>) -> Self {
        let size_hint = state.size_hint();
        let (tx, rx) = mpsc::channel(1);
        let join_handle = tokio::spawn(async move {
            stream_inflight_fill_body(state, tx).await;
        });
        Self { rx, size_hint, done: false, join_handle: Some(join_handle) }
    }
}

impl Drop for InFlightFillBody {
    fn drop(&mut self) {
        if let Some(join_handle) = self.join_handle.take()
            && !join_handle.is_finished()
        {
            join_handle.abort();
        }
    }
}

impl hyper::body::Body for InFlightFillBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<std::result::Result<Frame<Self::Data>, Self::Error>>> {
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

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.size_hint.clone()
    }
}

async fn stream_inflight_fill_body(
    state: Arc<CacheFillReadState>,
    tx: mpsc::Sender<std::result::Result<Frame<Bytes>, BoxError>>,
) {
    let mut file = match open_inflight_fill_body_file(&state).await {
        Ok(file) => file,
        Err(error) => {
            let _ = tx.send(Err(error.into())).await;
            return;
        }
    };
    let mut offset = 0u64;

    loop {
        let notified = state.notify.notified();
        let available = state.bytes_written();
        if available > offset {
            let chunk_len =
                usize::try_from((available - offset).min(IN_FLIGHT_FILL_READ_CHUNK_BYTES as u64))
                    .expect("bounded read chunk length should fit in usize");
            let mut buffer = vec![0; chunk_len];
            let mut filled = 0usize;
            while filled < chunk_len {
                match file.read(&mut buffer[filled..]).await {
                    Ok(0) if state.is_finished() => {
                        let _ = tx
                            .send(Err(std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                "in-flight cache fill ended before announced bytes became readable",
                            )
                            .into()))
                            .await;
                        return;
                    }
                    Ok(0) => state.notify.notified().await,
                    Ok(read) => filled += read,
                    Err(error) => {
                        let _ = tx.send(Err(error.into())).await;
                        return;
                    }
                }
            }
            offset = offset.saturating_add(chunk_len as u64);
            if tx.send(Ok(Frame::data(Bytes::from(buffer)))).await.is_err() {
                return;
            }
            continue;
        }

        if let Some(error) = state.error_message() {
            let _ = tx
                .send(Err(std::io::Error::other(format!(
                    "failed to continue reading in-flight cache fill: {error}"
                ))
                .into()))
                .await;
            return;
        }

        if state.is_finished() {
            if let Some(trailers) = state.trailers() {
                let _ = tx.send(Ok(Frame::trailers(trailers))).await;
            }
            return;
        }

        notified.await;
    }
}

async fn open_inflight_fill_body_file(state: &CacheFillReadState) -> std::io::Result<File> {
    match File::open(&state.body_tmp_path).await {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && state.is_finished() => {
            File::open(&state.body_path).await
        }
        Err(error) => Err(error),
    }
}
