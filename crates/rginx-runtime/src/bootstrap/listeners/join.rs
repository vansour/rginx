use rginx_core::{Error, Result};

use super::{ListenerGroupMap, ListenerWorkerGroup};

pub(crate) async fn join_listener_worker_groups(
    http_state: &rginx_http::SharedState,
    active_listener_groups: &mut ListenerGroupMap,
    draining_listener_groups: &mut Vec<ListenerWorkerGroup>,
) -> Result<()> {
    for group in active_listener_groups.values_mut() {
        join_listener_worker_group(group).await?;
    }
    active_listener_groups.clear();

    for group in draining_listener_groups.iter_mut() {
        let listener_id = group.listener.id.clone();
        join_listener_worker_group(group).await?;
        http_state.remove_retired_listener_runtime(&listener_id).await;
    }
    draining_listener_groups.clear();

    Ok(())
}

pub(crate) async fn join_aborted_listener_worker_groups(
    http_state: &rginx_http::SharedState,
    active_listener_groups: &mut ListenerGroupMap,
    draining_listener_groups: &mut Vec<ListenerWorkerGroup>,
) {
    for group in active_listener_groups.values_mut() {
        join_aborted_listener_worker_group(group).await;
    }
    active_listener_groups.clear();

    for group in draining_listener_groups.iter_mut() {
        let listener_id = group.listener.id.clone();
        join_aborted_listener_worker_group(group).await;
        http_state.remove_retired_listener_runtime(&listener_id).await;
    }
    draining_listener_groups.clear();
}

pub(crate) async fn join_listener_worker_group(group: &mut ListenerWorkerGroup) -> Result<()> {
    let mut first_error = None;

    while group.joined_tasks < group.tasks.len() {
        let worker_index = group.joined_tasks;
        match (&mut group.tasks[worker_index]).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(Error::Server(format!(
                        "listener `{}` worker {worker_index} failed to join: {error}",
                        group.listener.name
                    )));
                }
            }
        }
        group.joined_tasks += 1;
    }
    group.tasks.clear();
    group.joined_tasks = 0;

    if let Some(error) = first_error {
        return Err(error);
    }

    Ok(())
}

async fn join_aborted_listener_worker_group(group: &mut ListenerWorkerGroup) {
    while group.joined_tasks < group.tasks.len() {
        if let Err(error) = (&mut group.tasks[group.joined_tasks]).await
            && !error.is_cancelled()
        {
            tracing::warn!(%error, listener = %group.listener.name, "http worker failed after abort");
        }
        group.joined_tasks += 1;
    }
    group.tasks.clear();
    group.joined_tasks = 0;
}
