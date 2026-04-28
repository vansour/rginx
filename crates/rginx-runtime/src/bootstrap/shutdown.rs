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
    admin_task: &mut Option<JoinHandle<std::io::Result<()>>>,
    cache_task: &mut Option<JoinHandle<()>>,
    health_task: &mut Option<JoinHandle<()>>,
    ocsp_task: &mut Option<JoinHandle<()>>,
) -> Result<()> {
    let _ = shutdown_tx.send(true);
    initiate_shutdown_for_groups(active_listener_groups.values());
    initiate_shutdown_for_groups(draining_listener_groups.iter());

    match tokio::time::timeout(shutdown_timeout, async {
        join_listener_worker_groups(&state.http, active_listener_groups, draining_listener_groups)
            .await?;
        join_admin_task(admin_task).await?;
        join_unit_task(cache_task, "cache cleanup").await?;
        join_unit_task(health_task, "active health").await?;
        join_unit_task(ocsp_task, "OCSP refresh").await?;
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
            abort_task(admin_task.as_ref());
            abort_task(cache_task.as_ref());
            abort_task(health_task.as_ref());
            abort_task(ocsp_task.as_ref());

            join_aborted_listener_worker_groups(
                &state.http,
                active_listener_groups,
                draining_listener_groups,
            )
            .await;

            join_admin_task_after_abort(admin_task).await;
            join_unit_task_after_abort(cache_task, "cache cleanup").await;
            join_unit_task_after_abort(health_task, "active health").await;
            join_unit_task_after_abort(ocsp_task, "OCSP refresh").await;

            state.http.abort_background_tasks().await;
            Ok(())
        }
    }
}

fn abort_task<T>(task: Option<&JoinHandle<T>>) {
    if let Some(task) = task {
        task.abort();
    }
}

async fn join_admin_task(task: &mut Option<JoinHandle<std::io::Result<()>>>) -> Result<()> {
    if let Some(task) = task.take() {
        task.await.map_err(|error| {
            Error::Server(format!("admin socket task failed to join: {error}"))
        })??;
    }
    Ok(())
}

async fn join_unit_task(task: &mut Option<JoinHandle<()>>, name: &str) -> Result<()> {
    if let Some(task) = task.take() {
        task.await
            .map_err(|error| Error::Server(format!("{name} task failed to join: {error}")))?;
    }
    Ok(())
}

async fn join_admin_task_after_abort(task: &mut Option<JoinHandle<std::io::Result<()>>>) {
    if let Some(task) = task.take() {
        match task.await {
            Err(error) if !error.is_cancelled() => {
                tracing::warn!(%error, "admin socket task failed after abort");
            }
            Ok(Err(error)) => {
                tracing::warn!(%error, "admin socket task returned error after abort");
            }
            _ => {}
        }
    }
}

async fn join_unit_task_after_abort(task: &mut Option<JoinHandle<()>>, name: &str) {
    if let Some(task) = task.take()
        && let Err(error) = task.await
        && !error.is_cancelled()
    {
        tracing::warn!(%error, "{name} task failed after abort");
    }
}

#[cfg(test)]
mod tests;
