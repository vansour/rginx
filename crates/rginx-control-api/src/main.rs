mod app;
mod auth;
mod config;
mod error;
mod middleware;
mod request_context;
mod routes;
mod state;

use anyhow::Context;
use rginx_control_service::{ControlPlaneServiceConfig, ControlPlaneServices};
use rginx_control_store::{ControlPlaneStore, ControlPlaneStoreConfig};
use tokio::net::TcpListener;

use crate::config::ControlApiConfig;
use crate::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rginx_observability::init_logging()
        .map_err(|error| anyhow::anyhow!("failed to initialize logging: {error}"))?;

    let config = ControlApiConfig::from_env()?;
    let store = ControlPlaneStore::new(ControlPlaneStoreConfig::from_env()?);
    let services = ControlPlaneServices::new(store, ControlPlaneServiceConfig::for_api()?);
    let state = AppState::new(
        config.bind_addr,
        config.agent_shared_token.clone(),
        config.ui_dir.clone(),
        services,
    );
    let app = app::build_router(state);
    let listener = TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("failed to bind control API on {}", config.bind_addr))?;

    tracing::info!(
        bind_addr = %config.bind_addr,
        service = "rginx-control-api",
        "control plane API is ready"
    );

    axum::serve(listener, app).await.context("control plane API stopped unexpectedly")
}
