mod cli;

#[cfg(not(target_os = "linux"))]
compile_error!("rginx supports Linux only");

use anyhow::{Context, anyhow};
use clap::Parser;

use crate::cli::{Cli, Command};

fn main() -> anyhow::Result<()> {
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
                "configuration is valid: listen={} tls={} vhosts={} routes={} upstreams={} worker_threads={} accept_workers={}",
                config.server.listen_addr,
                if config.tls_enabled() { "enabled" } else { "disabled" },
                config.total_vhost_count(),
                config.total_route_count(),
                config.upstreams.len(),
                config
                    .runtime
                    .worker_threads
                    .map(|count| count.to_string())
                    .unwrap_or_else(|| "auto".to_string()),
                config.runtime.accept_workers,
            );
            Ok(())
        }
        None => {
            let runtime = build_runtime(config.runtime.worker_threads)
                .context("failed to construct tokio runtime")?;
            runtime
                .block_on(rginx_runtime::run(cli.config, config))
                .context("runtime exited with an error")
        }
    }
}

fn build_runtime(worker_threads: Option<usize>) -> anyhow::Result<tokio::runtime::Runtime> {
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    if let Some(worker_threads) = worker_threads {
        builder.worker_threads(worker_threads);
    }
    builder.build().map_err(|error| anyhow!("failed to build tokio runtime: {error}"))
}
