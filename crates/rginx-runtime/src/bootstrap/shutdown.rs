use std::time::Duration;

use rginx_core::{Error, Result};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::state::RuntimeState;

use super::listeners::{
    ListenerGroupMap, ListenerWorkerGroup, abort_listener_worker_groups,
    initiate_shutdown_for_groups, join_aborted_listener_worker_groups, join_listener_worker_groups,
};

pub(super) async fn graceful_shutdown(
    state: &RuntimeState,
    shutdown_timeout: Duration,
    shutdown_tx: &watch::Sender<bool>,
    active_listener_groups: &mut ListenerGroupMap,
    draining_listener_groups: &mut Vec<ListenerWorkerGroup>,
    admin_task: &mut JoinHandle<std::io::Result<()>>,
    health_task: &mut JoinHandle<()>,
    ocsp_task: &mut JoinHandle<()>,
) -> Result<()> {
    let _ = shutdown_tx.send(true);
    initiate_shutdown_for_groups(active_listener_groups.values());
    initiate_shutdown_for_groups(draining_listener_groups.iter());

    match tokio::time::timeout(shutdown_timeout, async {
        join_listener_worker_groups(&state.http, active_listener_groups, draining_listener_groups)
            .await?;
        (&mut *admin_task).await.map_err(|error| {
            Error::Server(format!("admin socket task failed to join: {error}"))
        })??;
        (&mut *health_task).await.map_err(|error| {
            Error::Server(format!("active health task failed to join: {error}"))
        })?;
        (&mut *ocsp_task)
            .await
            .map_err(|error| Error::Server(format!("OCSP refresh task failed to join: {error}")))?;
        state.http.drain_background_tasks().await;
        Ok::<(), Error>(())
    })
    .await
    {
        Ok(join_result) => join_result,
        Err(_) => {
            tracing::warn!(
                "shutdown timeout reached before background tasks drained all active work"
            );
            abort_listener_worker_groups(active_listener_groups.values());
            abort_listener_worker_groups(draining_listener_groups.iter());
            admin_task.abort();
            health_task.abort();
            ocsp_task.abort();

            join_aborted_listener_worker_groups(
                &state.http,
                active_listener_groups,
                draining_listener_groups,
            )
            .await;

            if let Err(error) = (&mut *admin_task).await
                && !error.is_cancelled()
            {
                tracing::warn!(%error, "admin socket task failed after abort");
            }

            if let Err(error) = (&mut *health_task).await
                && !error.is_cancelled()
            {
                tracing::warn!(%error, "active health task failed after abort");
            }

            if let Err(error) = (&mut *ocsp_task).await
                && !error.is_cancelled()
            {
                tracing::warn!(%error, "OCSP refresh task failed after abort");
            }

            state.http.abort_background_tasks().await;
            Ok(())
        }
    }
}
