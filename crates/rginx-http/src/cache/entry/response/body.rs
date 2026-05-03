use crate::handler::BoxError;
use bytes::Bytes;
use hyper::body::{Body, Frame, SizeHint};
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const COMMITTED_CACHE_READ_CHUNK_BYTES: usize = 64 * 1024;

pub(in crate::cache) struct CachedFileBody {
    rx: mpsc::Receiver<std::result::Result<Frame<Bytes>, BoxError>>,
    size_hint: SizeHint,
    done: bool,
    join_handle: Option<JoinHandle<()>>,
}

impl CachedFileBody {
    pub(in crate::cache) fn new(file: File, body_size_bytes: usize) -> Self {
        let (tx, rx) = mpsc::channel(1);
        let join_handle = tokio::spawn(async move {
            stream_cached_file_body(file, body_size_bytes, tx).await;
        });
        let mut size_hint = SizeHint::default();
        size_hint.set_exact(body_size_bytes as u64);
        Self { rx, size_hint, done: false, join_handle: Some(join_handle) }
    }
}

impl Drop for CachedFileBody {
    fn drop(&mut self) {
        if let Some(join_handle) = self.join_handle.take()
            && !join_handle.is_finished()
        {
            join_handle.abort();
        }
    }
}

impl Body for CachedFileBody {
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

async fn stream_cached_file_body(
    mut file: File,
    mut remaining_bytes: usize,
    tx: mpsc::Sender<std::result::Result<Frame<Bytes>, BoxError>>,
) {
    while remaining_bytes > 0 {
        let chunk_len = remaining_bytes.min(COMMITTED_CACHE_READ_CHUNK_BYTES);
        let mut buffer = vec![0; chunk_len];
        let mut filled = 0usize;
        while filled < chunk_len {
            match file.read(&mut buffer[filled..]).await {
                Ok(0) => {
                    let _ = tx
                        .send(Err(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "cached body ended before the committed metadata body length was read",
                        )
                        .into()))
                        .await;
                    return;
                }
                Ok(read) => filled += read,
                Err(error) => {
                    let _ = tx.send(Err(error.into())).await;
                    return;
                }
            }
        }
        remaining_bytes -= chunk_len;
        if tx.send(Ok(Frame::data(Bytes::from(buffer)))).await.is_err() {
            return;
        }
    }
}
