use std::collections::HashMap;
use std::sync::Arc;

use rginx_core::ConfigSnapshot;
use tokio::sync::Notify;

use super::activate::activate_prepared_listener_worker_group;
use super::join::join_listener_worker_group;
use super::{ListenerGroupMap, ListenerWorkerGroup, PreparedListenerWorkerGroup};

pub(crate) async fn reconcile_listener_worker_groups(
    http_state: &rginx_http::SharedState,
    next_config: &ConfigSnapshot,
    prepared_additions: Vec<PreparedListenerWorkerGroup>,
    active_listener_groups: &mut ListenerGroupMap,
    draining_listener_groups: &mut Vec<ListenerWorkerGroup>,
    drain_completion_notify: Arc<Notify>,
) {
    let next_by_id = next_config
        .listeners
        .iter()
        .map(|listener| (listener.id.clone(), listener.clone()))
        .collect::<HashMap<_, _>>();

    let removed_ids = active_listener_groups
        .keys()
        .filter(|listener_id| !next_by_id.contains_key(*listener_id))
        .cloned()
        .collect::<Vec<_>>();

    for removed_id in removed_ids {
        if let Some(group) = active_listener_groups.remove(&removed_id) {
            http_state.retire_listener_runtime(&group.listener);
            group.initiate_shutdown();
            tracing::info!(
                listener = %group.listener.name,
                listen = %group.listener.server.listen_addr,
                "listener removed from active config; draining existing connections"
            );
            draining_listener_groups.push(group);
        }
    }

    for (listener_id, group) in active_listener_groups.iter_mut() {
        let next_listener = next_by_id
            .get(listener_id)
            .expect("active listener ids should remain present in the next config");
        group.listener = next_listener.clone();
    }

    for prepared in prepared_additions {
        let listener_id = prepared.listener.id.clone();
        let group = activate_prepared_listener_worker_group(
            prepared,
            http_state.clone(),
            drain_completion_notify.clone(),
        );
        active_listener_groups.insert(listener_id, group);
    }

    prune_draining_listener_groups(http_state, draining_listener_groups).await;
}

pub(crate) async fn prune_draining_listener_groups(
    http_state: &rginx_http::SharedState,
    draining_listener_groups: &mut Vec<ListenerWorkerGroup>,
) {
    let mut index = 0usize;
    while index < draining_listener_groups.len() {
        if !draining_listener_groups[index].is_finished() {
            index += 1;
            continue;
        }

        let mut group = draining_listener_groups.remove(index);
        let listener_id = group.listener.id.clone();
        if let Err(error) = join_listener_worker_group(&mut group).await {
            tracing::warn!(%error, listener_id = %listener_id, "listener drain failed");
        }
        http_state.remove_retired_listener_runtime(&listener_id).await;
    }
}
