use rginx_control_service::ControlPlaneServices;

use crate::config::ControlWorkerConfig;

pub async fn run(
    config: ControlWorkerConfig,
    services: ControlPlaneServices,
) -> anyhow::Result<()> {
    tracing::info!(
        concurrency = config.concurrency,
        poll_interval_secs = config.poll_interval.as_secs(),
        "control worker started"
    );

    let mut ticker = tokio::time::interval(config.poll_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                match services.worker().collect_tick_report().await {
                    Ok(report) => {
                        tracing::info!(
                            concurrency = config.concurrency,
                            service = %report.service_name,
                            known_nodes = report.known_nodes,
                            active_deployments = report.active_deployments,
                            offline_reconciled_nodes = report.offline_reconciled_nodes,
                            dispatched_targets = report.dispatched_targets,
                            finalized_deployments = report.finalized_deployments,
                            rollback_deployments_created = report.rollback_deployments_created,
                            postgres = %report.postgres_endpoint,
                            dragonfly = %report.dragonfly_endpoint,
                            "control worker heartbeat"
                        );
                    }
                    Err(error) => {
                        tracing::warn!(
                            concurrency = config.concurrency,
                            error = %error,
                            "control worker failed to collect runtime context"
                        );
                    }
                }
            }
            result = tokio::signal::ctrl_c() => {
                result?;
                tracing::info!("control worker received shutdown signal");
                return Ok(());
            }
        }
    }
}
