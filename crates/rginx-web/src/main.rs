mod app;
mod auth;
mod config;
mod dns_runtime;
mod error;
mod middleware;
mod request_context;
mod routes;
mod state;
mod worker;

use anyhow::Context;
use rginx_control_service::{ControlPlaneServiceConfig, ControlPlaneServices};
use rginx_control_store::{ControlPlaneStore, ControlPlaneStoreConfig};
use rginx_dns::serve as serve_dns;
use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::config::RginxWebConfig;
use crate::dns_runtime::DnsRuntimeManager;
use crate::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rginx_observability::init_logging()
        .map_err(|error| anyhow::anyhow!("failed to initialize logging: {error}"))?;

    let config = RginxWebConfig::from_env()?;
    let store = ControlPlaneStore::new(ControlPlaneStoreConfig::from_env()?);
    store.bootstrap().await.context("failed to bootstrap control-plane database")?;
    let dns_runtime = std::sync::Arc::new(DnsRuntimeManager::new(
        store.clone(),
        config.dns_udp_bind_addr,
        config.dns_tcp_bind_addr,
    ));
    dns_runtime.refresh().await.context("failed to prime dns runtime state")?;
    let services = ControlPlaneServices::new(store, ControlPlaneServiceConfig::for_web()?);
    let state = AppState::new(
        config.bind_addr,
        config.agent_shared_token.clone(),
        config.ui_dir.clone(),
        services.clone(),
        dns_runtime.enabled().then_some(dns_runtime.clone()),
    );
    let app = app::build_router(state);
    let listener = TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("failed to bind rginx web on {}", config.bind_addr))?;

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let server_shutdown_rx = shutdown_rx.clone();
    let worker_shutdown_rx = shutdown_rx.clone();
    let dns_shutdown_rx = shutdown_rx.clone();
    let dns_refresh_shutdown_rx = shutdown_rx.clone();

    let mut server_task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(wait_for_shutdown(server_shutdown_rx))
            .await
            .context("rginx web HTTP server stopped unexpectedly")
    });
    let mut worker_task = tokio::spawn(async move {
        worker::run(config.poll_interval, services, worker_shutdown_rx).await
    });
    let mut dns_task = if dns_runtime.enabled() {
        let manager = dns_runtime.clone();
        Some(tokio::spawn(async move {
            serve_dns(manager.server_config(), manager, dns_shutdown_rx)
                .await
                .context("rginx web authoritative dns server stopped unexpectedly")
        }))
    } else {
        None
    };
    let mut dns_refresh_task = if dns_runtime.enabled() {
        let manager = dns_runtime.clone();
        Some(tokio::spawn(async move {
            manager
                .run_refresh_loop(config.poll_interval, dns_refresh_shutdown_rx)
                .await
                .context("rginx web dns refresh loop stopped unexpectedly")
        }))
    } else {
        None
    };

    tracing::info!(
        bind_addr = %config.bind_addr,
        poll_interval_secs = config.poll_interval.as_secs(),
        dns_udp_bind_addr = ?config.dns_udp_bind_addr,
        dns_tcp_bind_addr = ?config.dns_tcp_bind_addr,
        service = "rginx-web",
        "rginx web is ready"
    );

    tokio::select! {
        result = tokio::signal::ctrl_c() => {
            result.context("failed to listen for shutdown signal")?;
            tracing::info!("rginx web received shutdown signal");
        }
        result = &mut server_task => {
            let _ = shutdown_tx.send(true);
            return flatten_task_result(result, "rginx web HTTP task");
        }
        result = &mut worker_task => {
            let _ = shutdown_tx.send(true);
            return flatten_task_result(result, "rginx web background worker");
        }
        result = wait_optional_task(&mut dns_task) => {
            let _ = shutdown_tx.send(true);
            return flatten_task_result(result, "rginx web dns task");
        }
        result = wait_optional_task(&mut dns_refresh_task) => {
            let _ = shutdown_tx.send(true);
            return flatten_task_result(result, "rginx web dns refresh task");
        }
    }

    let _ = shutdown_tx.send(true);
    flatten_task_result(server_task.await, "rginx web HTTP task")?;
    flatten_task_result(worker_task.await, "rginx web background worker")?;
    if let Some(task) = dns_task {
        flatten_task_result(task.await, "rginx web dns task")?;
    }
    if let Some(task) = dns_refresh_task {
        flatten_task_result(task.await, "rginx web dns refresh task")?;
    }
    Ok(())
}

async fn wait_for_shutdown(mut shutdown_rx: watch::Receiver<bool>) {
    if *shutdown_rx.borrow() {
        return;
    }
    let _ = shutdown_rx.changed().await;
}

fn flatten_task_result(
    result: Result<anyhow::Result<()>, tokio::task::JoinError>,
    task_name: &str,
) -> anyhow::Result<()> {
    match result {
        Ok(inner) => inner,
        Err(error) => Err(anyhow::anyhow!("{task_name} failed to join: {error}")),
    }
}

async fn wait_optional_task(
    task: &mut Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
) -> Result<anyhow::Result<()>, tokio::task::JoinError> {
    match task {
        Some(task) => task.await,
        None => std::future::pending().await,
    }
}
