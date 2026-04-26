use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bytes::Bytes;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;

pub(super) type H3SendRequest = h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>;
pub(super) type H3RequestStream = h3::client::RequestStream<h3_quinn::BidiStream<Bytes>, Bytes>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct Http3SessionKey {
    pub(super) remote_addr: SocketAddr,
    pub(super) server_name: String,
}

pub(super) struct Http3Session {
    sender: Mutex<H3SendRequest>,
    closed: Arc<AtomicBool>,
    driver_task: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone)]
pub(super) enum Http3SessionEntry {
    Ready(Arc<Http3Session>),
    Pending(Arc<Notify>),
}

impl Http3Session {
    pub(super) fn new(sender: H3SendRequest) -> Self {
        Self {
            sender: Mutex::new(sender),
            closed: Arc::new(AtomicBool::new(false)),
            driver_task: Mutex::new(None),
        }
    }

    pub(super) async fn sender(&self) -> H3SendRequest {
        self.sender.lock().await.clone()
    }

    pub(super) async fn set_driver_task(&self, task: JoinHandle<()>) {
        *self.driver_task.lock().await = Some(task);
    }

    pub(super) fn close_flag(&self) -> Arc<AtomicBool> {
        self.closed.clone()
    }

    pub(super) fn mark_closed(&self) {
        self.closed.store(true, Ordering::Release);
    }

    pub(super) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

impl Drop for Http3Session {
    fn drop(&mut self) {
        if let Ok(mut task) = self.driver_task.try_lock()
            && let Some(task) = task.take()
            && !task.is_finished()
        {
            task.abort();
        }
    }
}
