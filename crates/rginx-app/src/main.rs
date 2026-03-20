mod cli;

use anyhow::{anyhow, Context};
use clap::Parser;

use crate::cli::Cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    rginx_observability::init_logging()
        .map_err(|error| anyhow!("failed to initialize logging: {error}"))?;

    let config = rginx_config::load_and_compile(&cli.config)
        .with_context(|| format!("failed to load {}", cli.config.display()))?;

    rginx_runtime::run(config).await.context("runtime exited with an error")
}
