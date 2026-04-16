mod config;
mod worker;

use rginx_control_service::{ControlPlaneServiceConfig, ControlPlaneServices};
use rginx_control_store::{ControlPlaneStore, ControlPlaneStoreConfig};

use crate::config::ControlWorkerConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rginx_observability::init_logging()
        .map_err(|error| anyhow::anyhow!("failed to initialize logging: {error}"))?;

    let config = ControlWorkerConfig::from_env()?;
    let store = ControlPlaneStore::new(ControlPlaneStoreConfig::from_env()?);
    let services = ControlPlaneServices::new(store, ControlPlaneServiceConfig::for_worker());

    worker::run(config, services).await
}
