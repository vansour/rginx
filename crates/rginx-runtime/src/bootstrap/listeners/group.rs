use std::net::{TcpListener as StdTcpListener, UdpSocket as StdUdpSocket};
use std::sync::Arc;

use rginx_core::{Listener, Result};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::restart::ListenerHandle;

pub(crate) struct ListenerWorkerGroup {
    pub(super) listener: Listener,
    pub(super) std_listener: Arc<StdTcpListener>,
    pub(super) std_udp_sockets: Vec<Arc<StdUdpSocket>>,
    pub(super) shutdown_tx: watch::Sender<bool>,
    pub(super) tasks: Vec<JoinHandle<Result<()>>>,
    pub(super) joined_tasks: usize,
}

impl ListenerWorkerGroup {
    pub(crate) fn restart_handle(&self) -> ListenerHandle {
        ListenerHandle {
            listener: self.listener.clone(),
            std_listener: self.std_listener.clone(),
            std_udp_sockets: self.std_udp_sockets.clone(),
        }
    }

    pub(crate) fn initiate_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    pub(crate) fn abort(&self) {
        for task in &self.tasks {
            task.abort();
        }
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.tasks.iter().all(JoinHandle::is_finished)
    }
}
