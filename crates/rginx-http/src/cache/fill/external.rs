use bytes::Bytes;
use hyper::body::Frame;
use tokio::fs::{self, File};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::sleep;

use super::super::store::range::build_downstream_response;
use super::super::{CacheRequest, RouteCachePolicy};
use super::common::{
    EXTERNAL_FILL_POLL_INTERVAL, IN_FLIGHT_FILL_READ_CHUNK_BYTES, inflight_response_parts,
    size_hint_from_headers,
};
use super::shared::{
    ExternalCacheFillReadState, header_map_from_shared_headers, read_external_fill_state_record,
};
use crate::handler::{BoxError, full_body};

pub(in crate::cache) fn build_external_inflight_fill_response(
    state: ExternalCacheFillReadState,
    request: &CacheRequest,
    policy: &RouteCachePolicy,
    read_body: bool,
) -> std::io::Result<crate::handler::HttpResponse> {
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

struct ExternalInFlightFillBody {
    rx: mpsc::Receiver<std::result::Result<Frame<Bytes>, BoxError>>,
    size_hint: hyper::body::SizeHint,
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

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.size_hint.clone()
    }
}

async fn stream_external_fill_body(
    state: ExternalCacheFillReadState,
    tx: mpsc::Sender<std::result::Result<Frame<Bytes>, BoxError>>,
) {
    let mut current_state = match read_external_fill_state_record(&state.source) {
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
        match read_external_fill_state_record(&state.source) {
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
