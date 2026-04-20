use std::time::Duration;

use rginx_control_service::ControlPlaneServices;
use tokio::sync::watch;

pub async fn run(
    poll_interval: Duration,
    services: ControlPlaneServices,
    mut shutdown_rx: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    tracing::info!(
        poll_interval_secs = poll_interval.as_secs(),
        "rginx web background worker started"
    );

    let mut ticker = tokio::time::interval(poll_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                match services.worker().collect_tick_report().await {
                    Ok(report) => {
                        tracing::info!(
                            service = %report.service_name,
                            known_nodes = report.known_nodes,
                            active_deployments = report.active_deployments,
                            active_dns_deployments = report.active_dns_deployments,
                            offline_reconciled_nodes = report.offline_reconciled_nodes,
                            dispatched_targets = report.dispatched_targets,
                            dns_assigned_targets = report.dns_assigned_targets,
                            finalized_deployments = report.finalized_deployments,
                            finalized_dns_deployments = report.finalized_dns_deployments,
                            rollback_deployments_created = report.rollback_deployments_created,
                            dns_rollback_deployments_created = report.dns_rollback_deployments_created,
                            postgres = %report.postgres_endpoint,
                            dragonfly = %report.dragonfly_endpoint,
                            "rginx web background worker heartbeat"
                        );
                    }
                    Err(error) => {
                        tracing::warn!(
                            error = %error,
                            "rginx web background worker failed to collect runtime context"
                        );
                    }
                }
            }
            result = shutdown_rx.changed() => {
                if result.is_err() || *shutdown_rx.borrow() {
                    tracing::info!("rginx web background worker shutting down");
                    return Ok(());
                }
            }
        }
    }
}
