mod cli;

use anyhow::{Context, anyhow};
use clap::Parser;

use crate::cli::{Cli, Command};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    rginx_observability::init_logging()
        .map_err(|error| anyhow!("failed to initialize logging: {error}"))?;

    let config = rginx_config::load_and_compile(&cli.config)
        .with_context(|| format!("failed to load {}", cli.config.display()))?;

    match cli.command {
        Some(Command::Check) => {
            rginx_http::SharedState::from_config(config.clone())
                .context("failed to initialize runtime dependencies")?;

            println!(
                "configuration is valid: listen={} tls={} vhosts={} routes={} upstreams={}",
                config.server.listen_addr,
                if config.server.tls.is_some() { "enabled" } else { "disabled" },
                config.total_vhost_count(),
                config.total_route_count(),
                config.upstreams.len()
            );
            Ok(())
        }
        None => {
            rginx_runtime::run(cli.config, config).await.context("runtime exited with an error")
        }
    }
}
