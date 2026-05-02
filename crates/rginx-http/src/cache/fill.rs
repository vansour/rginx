use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Response, StatusCode};
use hyper::body::{Frame, SizeHint};
use serde::{Deserialize, Serialize};
use tokio::fs::{self, File};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep};

use super::entry::{downstream_range_trim_plan, unix_time_ms};
use super::shared::{run_blocking, shared_fill_lock_path, shared_fill_state_path};
use super::store::range::build_downstream_response;
use super::*;
use crate::handler::{BoxError, boxed_body, full_body};

const IN_FLIGHT_FILL_READ_CHUNK_BYTES: usize = 16 * 1024;
const EXTERNAL_FILL_POLL_INTERVAL: Duration = Duration::from_millis(10);

static SHARED_FILL_STATE_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub(super) struct SharedFillExternalStateHandle {
    lock_path: PathBuf,
    state_path: PathBuf,
    state: Arc<Mutex<SharedFillStateRecord>>,
}

#[derive(Clone)]
pub(super) struct ExternalCacheFillReadState {
    status: StatusCode,
    headers: HeaderMap,
    body_tmp_path: PathBuf,
    body_path: PathBuf,
    state_path: PathBuf,
    nonce: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SharedFillLockRecord {
    nonce: String,
    updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SharedFillStateRecord {
    nonce: String,
    response: Option<SharedFillResponseMetadata>,
    upstream_completed: bool,
    finished: bool,
    trailers: Option<Vec<SharedFillHeader>>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SharedFillResponseMetadata {
    status: u16,
    headers: Vec<SharedFillHeader>,
    body_tmp_path: PathBuf,
    body_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SharedFillHeader {
    name: String,
    value: Vec<u8>,
}

pub(super) struct CacheFillReadState {
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
    pub(super) fn new(
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

    pub(super) fn record_bytes_written(&self, body_size_bytes: usize) {
        self.bytes_written.store(body_size_bytes as u64, Ordering::Release);
        self.notify.notify_waiters();
        if let Some(external_state) = self.external_state.as_ref()
            && let Err(error) = external_state.heartbeat()
        {
            tracing::warn!(%error, "failed to heartbeat shared fill state");
        }
    }

    pub(super) fn finish(&self, trailers: Option<HeaderMap>) {
        *self.trailers.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = trailers.clone();
        self.finished.store(true, Ordering::Release);
        self.notify.notify_waiters();
        if let Some(external_state) = self.external_state.as_ref()
            && let Err(error) = external_state.finish(trailers)
        {
            tracing::warn!(%error, "failed to mark shared fill state complete");
        }
    }

    pub(super) fn fail(&self, error: impl std::fmt::Display) {
        let error = error.to_string();
        *self.error.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error.clone());
        self.notify.notify_waiters();
        if let Some(external_state) = self.external_state.as_ref()
            && let Err(external_error) = external_state.fail(&error)
        {
            tracing::warn!(%external_error, "failed to mark shared fill state failed");
        }
    }

    pub(super) fn can_serve(&self) -> bool {
        self.error.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref().is_none()
    }

    pub(super) fn can_share(&self) -> bool {
        self.can_serve() && !self.upstream_completed.load(Ordering::Acquire)
    }

    pub(super) fn mark_upstream_complete(&self) {
        self.upstream_completed.store(true, Ordering::Release);
        self.notify.notify_waiters();
        if let Some(external_state) = self.external_state.as_ref()
            && let Err(error) = external_state.mark_upstream_complete()
        {
            tracing::warn!(%error, "failed to mark shared fill state upstream-complete");
        }
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

    fn size_hint(&self) -> SizeHint {
        size_hint_from_headers(&self.headers)
    }
}

impl SharedFillExternalStateHandle {
    pub(super) fn create(
        zone: &rginx_core::CacheZone,
        key: &str,
        lock_path: &Path,
        now: u64,
    ) -> std::io::Result<Self> {
        let state_path = shared_fill_state_path(zone, key);
        let state = SharedFillStateRecord {
            nonce: next_shared_fill_nonce(now),
            response: None,
            upstream_completed: false,
            finished: false,
            trailers: None,
            error: None,
        };
        let handle = Self {
            lock_path: lock_path.to_path_buf(),
            state_path,
            state: Arc::new(Mutex::new(state)),
        };
        handle.persist_lock_and_state(now)?;
        Ok(handle)
    }

    fn publish_response(
        &self,
        status: StatusCode,
        headers: &HeaderMap,
        body_tmp_path: &Path,
        body_path: &Path,
    ) -> std::io::Result<()> {
        self.update_state(|state| {
            state.response = Some(SharedFillResponseMetadata {
                status: status.as_u16(),
                headers: shared_headers_from_map(headers),
                body_tmp_path: body_tmp_path.to_path_buf(),
                body_path: body_path.to_path_buf(),
            });
        })
    }

    fn mark_upstream_complete(&self) -> std::io::Result<()> {
        self.update_state(|state| {
            state.upstream_completed = true;
        })
    }

    fn finish(&self, trailers: Option<HeaderMap>) -> std::io::Result<()> {
        self.update_state(|state| {
            state.upstream_completed = true;
            state.finished = true;
            state.trailers = trailers.as_ref().map(shared_headers_from_map);
        })
    }

    fn fail(&self, error: impl std::fmt::Display) -> std::io::Result<()> {
        let error = error.to_string();
        self.update_state(move |state| {
            state.error = Some(error.clone());
        })
    }

    fn heartbeat(&self) -> std::io::Result<()> {
        run_blocking(|| {
            let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            persist_shared_fill_lock_record(
                &self.lock_path,
                &SharedFillLockRecord {
                    nonce: state.nonce.clone(),
                    updated_at_unix_ms: unix_time_ms(SystemTime::now()),
                },
            )
        })
    }

    fn persist_lock_and_state(&self, now: u64) -> std::io::Result<()> {
        run_blocking(|| {
            let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            persist_shared_fill_state_record(&self.state_path, &state)?;
            persist_shared_fill_lock_record(
                &self.lock_path,
                &SharedFillLockRecord { nonce: state.nonce.clone(), updated_at_unix_ms: now },
            )
        })
    }

    fn update_state(&self, update: impl FnOnce(&mut SharedFillStateRecord)) -> std::io::Result<()> {
        run_blocking(|| {
            let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            update(&mut state);
            persist_shared_fill_state_record(&self.state_path, &state)?;
            persist_shared_fill_lock_record(
                &self.lock_path,
                &SharedFillLockRecord {
                    nonce: state.nonce.clone(),
                    updated_at_unix_ms: unix_time_ms(SystemTime::now()),
                },
            )
        })
    }
}

pub(super) fn create_shared_external_fill_handle(
    zone: &rginx_core::CacheZone,
    key: &str,
    lock_path: &Path,
    now: u64,
) -> std::io::Result<SharedFillExternalStateHandle> {
    SharedFillExternalStateHandle::create(zone, key, lock_path, now)
}

pub(super) fn load_external_fill_state(
    zone: &rginx_core::CacheZone,
    key: &str,
) -> Option<ExternalCacheFillReadState> {
    let lock_path = shared_fill_lock_path(zone, key);
    let state_path = shared_fill_state_path(zone, key);
    let lock = read_shared_fill_lock_record(&lock_path).ok()?;
    let state = read_shared_fill_state_record(&state_path).ok()?;
    if state.nonce != lock.nonce {
        return None;
    }
    if state.error.is_some() || state.upstream_completed {
        return None;
    }
    let response = state.response?;
    let status = StatusCode::from_u16(response.status).ok()?;
    let headers = header_map_from_shared_headers(&response.headers).ok()?;
    Some(ExternalCacheFillReadState {
        status,
        headers,
        body_tmp_path: response.body_tmp_path,
        body_path: response.body_path,
        state_path,
        nonce: state.nonce,
    })
}

pub(super) fn build_inflight_fill_response(
    state: Arc<CacheFillReadState>,
    request: &CacheRequest,
    policy: &RouteCachePolicy,
    read_body: bool,
) -> std::io::Result<HttpResponse> {
    let (parts, trim_plan) =
        inflight_response_parts(state.status, &state.headers, request, policy)?;
    if read_body {
        return Ok(build_downstream_response(parts, InFlightFillBody::new(state), trim_plan));
    }
    Ok(build_downstream_response(parts, full_body(Bytes::new()), trim_plan))
}

pub(super) fn build_external_inflight_fill_response(
    state: ExternalCacheFillReadState,
    request: &CacheRequest,
    policy: &RouteCachePolicy,
    read_body: bool,
) -> std::io::Result<HttpResponse> {
    let (parts, trim_plan) =
        inflight_response_parts(state.status, &state.headers, request, policy)?;
    if read_body {
        return Ok(build_downstream_response(
            parts,
            ExternalInFlightFillBody::new(state),
            trim_plan,
        ));
    }
    Ok(build_downstream_response(parts, full_body(Bytes::new()), trim_plan))
}

pub(super) fn inflight_fill_body(state: Arc<CacheFillReadState>) -> HttpBody {
    boxed_body(InFlightFillBody::new(state))
}

struct InFlightFillBody {
    rx: mpsc::Receiver<std::result::Result<Frame<Bytes>, BoxError>>,
    size_hint: SizeHint,
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

    fn size_hint(&self) -> SizeHint {
        self.size_hint.clone()
    }
}

struct ExternalInFlightFillBody {
    rx: mpsc::Receiver<std::result::Result<Frame<Bytes>, BoxError>>,
    size_hint: SizeHint,
    done: bool,
    join_handle: Option<JoinHandle<()>>,
}

impl ExternalInFlightFillBody {
    fn new(state: ExternalCacheFillReadState) -> Self {
        let size_hint = size_hint_from_headers(&state.headers);
        let (tx, rx) = mpsc::channel(1);
        let join_handle = tokio::spawn(async move {
            stream_external_fill_body(state, tx).await;
        });
        Self { rx, size_hint, done: false, join_handle: Some(join_handle) }
    }
}

impl Drop for ExternalInFlightFillBody {
    fn drop(&mut self) {
        if let Some(join_handle) = self.join_handle.take()
            && !join_handle.is_finished()
        {
            join_handle.abort();
        }
    }
}

impl hyper::body::Body for ExternalInFlightFillBody {
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

    fn size_hint(&self) -> SizeHint {
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

async fn stream_external_fill_body(
    state: ExternalCacheFillReadState,
    tx: mpsc::Sender<std::result::Result<Frame<Bytes>, BoxError>>,
) {
    let mut current_state =
        match read_shared_fill_state_record_for_nonce(&state.state_path, &state.nonce) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let _ = tx.send(Err(error.into())).await;
                return;
            }
        };
    let mut file = match open_external_fill_body_file(&state, current_state.finished).await {
        Ok(file) => file,
        Err(error) => {
            let _ = tx.send(Err(error.into())).await;
            return;
        }
    };
    let mut offset = 0u64;

    loop {
        match read_shared_fill_state_record_for_nonce(&state.state_path, &state.nonce) {
            Ok(snapshot) => current_state = snapshot,
            Err(error) if current_state.finished || current_state.error.is_some() => {
                tracing::debug!(%error, "shared fill state disappeared after completion");
            }
            Err(_) => {}
        }

        let available = match external_fill_available_bytes(&state, current_state.finished).await {
            Ok(available) => available,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => {
                let _ = tx.send(Err(error.into())).await;
                return;
            }
        };

        if available > offset {
            let chunk_len =
                usize::try_from((available - offset).min(IN_FLIGHT_FILL_READ_CHUNK_BYTES as u64))
                    .expect("bounded read chunk length should fit in usize");
            let mut buffer = vec![0; chunk_len];
            let mut filled = 0usize;
            while filled < chunk_len {
                match file.read(&mut buffer[filled..]).await {
                    Ok(0) if current_state.finished => {
                        let _ = tx
                            .send(Err(std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                "external in-flight cache fill ended before readable bytes were visible",
                            )
                            .into()))
                            .await;
                        return;
                    }
                    Ok(0) => sleep(EXTERNAL_FILL_POLL_INTERVAL).await,
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

        if let Some(error) = current_state.error.as_ref() {
            let _ = tx
                .send(Err(std::io::Error::other(format!(
                    "failed to continue reading external in-flight cache fill: {error}"
                ))
                .into()))
                .await;
            return;
        }

        if current_state.finished {
            if let Some(trailers) = current_state
                .trailers
                .as_ref()
                .and_then(|trailers| header_map_from_shared_headers(trailers).ok())
            {
                let _ = tx.send(Ok(Frame::trailers(trailers))).await;
            }
            return;
        }

        sleep(EXTERNAL_FILL_POLL_INTERVAL).await;
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

async fn open_external_fill_body_file(
    state: &ExternalCacheFillReadState,
    finished: bool,
) -> std::io::Result<File> {
    match File::open(&state.body_tmp_path).await {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && finished => {
            File::open(&state.body_path).await
        }
        Err(error) => Err(error),
    }
}

async fn external_fill_available_bytes(
    state: &ExternalCacheFillReadState,
    finished: bool,
) -> std::io::Result<u64> {
    match fs::metadata(&state.body_tmp_path).await {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && finished => {
            Ok(fs::metadata(&state.body_path).await?.len())
        }
        Err(error) => Err(error),
    }
}

fn inflight_response_parts(
    status: StatusCode,
    headers: &HeaderMap,
    request: &CacheRequest,
    policy: &RouteCachePolicy,
) -> std::io::Result<(http::response::Parts, Option<super::entry::DownstreamRangeTrimPlan>)> {
    let trim_plan = downstream_range_trim_plan(status, headers, request, policy)?;
    let mut response = Response::builder().status(status);
    *response.headers_mut().expect("response builder should expose headers") = headers.clone();
    let (parts, _) = response
        .body(full_body(Bytes::new()))
        .map_err(|error| std::io::Error::other(error.to_string()))?
        .into_parts();
    Ok((parts, trim_plan))
}

fn size_hint_from_headers(headers: &HeaderMap) -> SizeHint {
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

fn shared_headers_from_map(headers: &HeaderMap) -> Vec<SharedFillHeader> {
    headers
        .iter()
        .map(|(name, value)| SharedFillHeader {
            name: name.as_str().to_string(),
            value: value.as_bytes().to_vec(),
        })
        .collect()
}

fn header_map_from_shared_headers(headers: &[SharedFillHeader]) -> std::io::Result<HeaderMap> {
    let mut map = HeaderMap::new();
    for header in headers {
        let name = HeaderName::from_bytes(header.name.as_bytes())
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let value = HeaderValue::from_bytes(&header.value)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        map.append(name, value);
    }
    Ok(map)
}

fn read_shared_fill_lock_record(path: &Path) -> std::io::Result<SharedFillLockRecord> {
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn read_shared_fill_state_record(path: &Path) -> std::io::Result<SharedFillStateRecord> {
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn read_shared_fill_state_record_for_nonce(
    path: &Path,
    nonce: &str,
) -> std::io::Result<SharedFillStateRecord> {
    let state = read_shared_fill_state_record(path)?;
    if state.nonce != nonce {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "shared fill state nonce mismatch: expected `{nonce}`, found `{}`",
                state.nonce
            ),
        ));
    }
    Ok(state)
}

fn persist_shared_fill_lock_record(
    path: &Path,
    record: &SharedFillLockRecord,
) -> std::io::Result<()> {
    atomic_write_json(path, record)
}

fn persist_shared_fill_state_record(
    path: &Path,
    record: &SharedFillStateRecord,
) -> std::io::Result<()> {
    atomic_write_json(path, record)
}

fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let bytes =
        serde_json::to_vec(value).map_err(|error| std::io::Error::other(error.to_string()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp_path = temp_json_path(path);
    std::fs::write(&tmp_path, bytes)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

fn temp_json_path(path: &Path) -> PathBuf {
    let counter = SHARED_FILL_STATE_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut file_name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| "shared-fill-state".into());
    file_name.push(format!(".tmp-{}-{counter}", std::process::id()));
    path.with_file_name(file_name)
}

fn next_shared_fill_nonce(now: u64) -> String {
    let counter = SHARED_FILL_STATE_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{now}-{}-{counter}", std::process::id())
}
