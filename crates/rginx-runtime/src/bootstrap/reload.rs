use std::sync::Arc;

use tokio::sync::Notify;

use crate::reload as runtime_reload;
use crate::state::RuntimeState;

use super::listeners::{
    ListenerGroupMap, ListenerWorkerGroup, prepare_added_listener_bindings,
    reconcile_listener_worker_groups,
};

pub(super) async fn handle_reload_signal(
    state: &RuntimeState,
    active_listener_groups: &mut ListenerGroupMap,
    draining_listener_groups: &mut Vec<ListenerWorkerGroup>,
    drain_completion_notify: Arc<Notify>,
) {
    match runtime_reload::prepare_reload(state).await {
        Ok(pending) => match prepare_added_listener_bindings(
            &pending.next_config.listeners,
            pending.next_config.runtime.accept_workers,
            active_listener_groups,
            draining_listener_groups,
        ) {
            Ok(prepared_additions) => match runtime_reload::commit_reload(state, pending).await {
                Ok(result) => {
                    reconcile_listener_worker_groups(
                        &state.http,
                        &result.config,
                        prepared_additions,
                        active_listener_groups,
                        draining_listener_groups,
                        drain_completion_notify,
                    )
                    .await;
                    tracing::info!(
                        revision = result.revision,
                        listeners = result.config.total_listener_count(),
                        vhosts = result.config.total_vhost_count(),
                        routes = result.config.total_route_count(),
                        upstreams = result.config.upstreams.len(),
                        "configuration reloaded"
                    );
                }
                Err(error) => {
                    tracing::warn!(%error, "configuration reload failed");
                }
            },
            Err(error) => {
                state.http.record_reload_failure(error.to_string(), pending.current_revision);
                tracing::warn!(%error, "configuration reload failed");
            }
        },
        Err(error) => {
            tracing::warn!(%error, "configuration reload failed");
        }
    }
}
