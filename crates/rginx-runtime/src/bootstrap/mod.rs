use std::path::PathBuf;
use std::sync::Arc;

use rginx_core::{ConfigSnapshot, Result};
use tokio::sync::{Notify, watch};

use crate::admin;
use crate::health;
use crate::ocsp;
use crate::shutdown as runtime_shutdown;
use crate::state::RuntimeState;

mod listeners;
mod reload;
mod restart;
mod shutdown;

use listeners::{
    ListenerWorkerGroup, build_initial_listener_groups, prune_draining_listener_groups,
};

pub async fn run(config_path: PathBuf, config: ConfigSnapshot) -> Result<()> {
    let state = RuntimeState::new(config_path, config)?;
    let current_config = state.current_config().await;
    let inherited_listeners = crate::restart::take_inherited_listeners_from_env()?;
    let drain_completion_notify = Arc::new(Notify::new());
    let mut active_listener_groups = build_initial_listener_groups(
        &current_config.listeners,
        current_config.runtime.accept_workers,
        inherited_listeners,
        state.http.clone(),
        drain_completion_notify.clone(),
    )
    .await?;
    let mut draining_listener_groups = Vec::<ListenerWorkerGroup>::new();
    let (shutdown_tx, _shutdown_rx) = watch::channel(false);

    tracing::info!(
        listeners = current_config.total_listener_count(),
        worker_threads = current_config.runtime.worker_threads,
        accept_workers = current_config.runtime.accept_workers,
        vhosts = current_config.total_vhost_count(),
        routes = current_config.total_route_count(),
        "starting rginx runtime"
    );

    let mut admin_task = tokio::spawn(admin::run(
        state.config_path.clone(),
        state.http.clone(),
        shutdown_tx.subscribe(),
    ));
    let mut health_task = tokio::spawn(health::run(state.http.clone(), shutdown_tx.subscribe()));
    let mut ocsp_task = tokio::spawn(ocsp::run(state.http.clone(), shutdown_tx.subscribe()));
    crate::restart::notify_ready_if_requested()?;

    loop {
        let drain_completion = drain_completion_notify.notified();
        tokio::pin!(drain_completion);
        prune_draining_listener_groups(&state.http, &mut draining_listener_groups).await;
        let signal = if draining_listener_groups.is_empty() {
            runtime_shutdown::wait_for_signal().await?
        } else {
            tokio::select! {
                signal = runtime_shutdown::wait_for_signal() => signal?,
                _ = &mut drain_completion => {
                    continue;
                }
            }
        };

        match signal {
            runtime_shutdown::RuntimeSignal::Reload => {
                tracing::info!("reload signal received");
                reload::handle_reload_signal(
                    &state,
                    &mut active_listener_groups,
                    &mut draining_listener_groups,
                    drain_completion_notify.clone(),
                )
                .await;
            }
            runtime_shutdown::RuntimeSignal::Restart => {
                tracing::info!("restart signal received");
                if restart::handle_restart_signal(&state.config_path, &active_listener_groups).await
                {
                    break;
                }
            }
            runtime_shutdown::RuntimeSignal::Shutdown => break,
        }
    }

    let current_config = state.current_config().await;
    tracing::info!(
        timeout_secs = current_config.runtime.shutdown_timeout.as_secs(),
        "graceful shutdown requested"
    );
    shutdown::graceful_shutdown(
        &state,
        current_config.runtime.shutdown_timeout,
        &shutdown_tx,
        &mut active_listener_groups,
        &mut draining_listener_groups,
        &mut admin_task,
        &mut health_task,
        &mut ocsp_task,
    )
    .await
}
