use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::{Notify, watch};

use super::PreparedListenerWorkerGroup;
use super::group::ListenerWorkerGroup;

struct WorkerDrainGuard {
    remaining_workers: Arc<AtomicUsize>,
    drain_completion_notify: Arc<Notify>,
}

impl Drop for WorkerDrainGuard {
    fn drop(&mut self) {
        if self.remaining_workers.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.drain_completion_notify.notify_waiters();
        }
    }
}

pub(crate) fn activate_prepared_listener_worker_group(
    prepared: PreparedListenerWorkerGroup,
    http_state: rginx_http::SharedState,
    drain_completion_notify: Arc<Notify>,
) -> ListenerWorkerGroup {
    let (shutdown_tx, _shutdown_rx) = watch::channel(false);
    let mut tasks = Vec::new();
    let remaining_workers = Arc::new(AtomicUsize::new(
        prepared.worker_listeners.len() + prepared.http3_endpoints.len(),
    ));

    for (worker_index, listener_socket) in prepared.worker_listeners.into_iter().enumerate() {
        tracing::info!(
            listener = %prepared.listener.name,
            listen = %prepared.listener.server.listen_addr,
            worker = worker_index,
            "starting accept worker"
        );
        let listener_id = prepared.listener.id.clone();
        let http_state = http_state.clone();
        let shutdown = shutdown_tx.subscribe();
        let remaining_workers = remaining_workers.clone();
        let drain_completion_notify = drain_completion_notify.clone();
        tasks.push(tokio::spawn(async move {
            let _drain_guard = WorkerDrainGuard { remaining_workers, drain_completion_notify };
            rginx_http::serve(listener_socket, listener_id, http_state, shutdown).await
        }));
    }

    for (worker_index, endpoint) in prepared.http3_endpoints.into_iter().enumerate() {
        tracing::info!(
            listener = %prepared.listener.name,
            listen = %prepared
                .listener
                .http3
                .as_ref()
                .map(|http3| http3.listen_addr)
                .unwrap_or(prepared.listener.server.listen_addr),
            worker = worker_index,
            "starting http3 accept worker"
        );
        let listener_id = prepared.listener.id.clone();
        let http_state = http_state.clone();
        let shutdown = shutdown_tx.subscribe();
        let remaining_workers = remaining_workers.clone();
        let drain_completion_notify = drain_completion_notify.clone();
        tasks.push(tokio::spawn(async move {
            let _drain_guard = WorkerDrainGuard { remaining_workers, drain_completion_notify };
            rginx_http::server::serve_http3(endpoint, listener_id, http_state, shutdown).await
        }));
    }

    ListenerWorkerGroup {
        listener: prepared.listener,
        std_listener: prepared.std_listener,
        std_udp_sockets: prepared.std_udp_sockets,
        shutdown_tx,
        tasks,
        joined_tasks: 0,
    }
}
