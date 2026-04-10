use std::path::Path;

use crate::restart as runtime_restart;

use super::listeners::ListenerGroupMap;

pub(super) async fn handle_restart_signal(
    config_path: &Path,
    active_listener_groups: &ListenerGroupMap,
) -> bool {
    let handles =
        active_listener_groups.values().map(|group| group.restart_handle()).collect::<Vec<_>>();
    match runtime_restart::restart(config_path, &handles).await {
        Ok(()) => {
            tracing::info!("replacement process became ready; starting graceful handoff");
            true
        }
        Err(error) => {
            tracing::warn!(%error, "graceful restart failed");
            false
        }
    }
}
