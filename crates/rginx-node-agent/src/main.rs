mod agent;
mod config;

use crate::config::NodeAgentConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rginx_observability::init_logging()
        .map_err(|error| anyhow::anyhow!("failed to initialize logging: {error}"))?;

    let config = NodeAgentConfig::from_env()?;
    agent::run(config).await
}
