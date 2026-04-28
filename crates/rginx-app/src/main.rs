mod admin_cli;
mod check;
mod cli;
mod pid_file;
mod runtime;
mod signal;

#[cfg(not(target_os = "linux"))]
compile_error!("rginx supports Linux only");

use anyhow::{Context, anyhow};
use clap::Parser;

use crate::check::{build_check_summary, print_check_success};
use crate::cli::{Cli, Command, pid_path_for_config};
use crate::pid_file::PidFileGuard;
use crate::runtime::build_runtime;
use crate::signal::send_signal_from_pid_file;

fn main() -> anyhow::Result<()> {
    rginx_http::install_default_crypto_provider();
    let cli = Cli::parse();

    if cli.command.is_some() && cli.test_config {
        return Err(anyhow!("`-t` cannot be used together with subcommands"));
    }

    if cli.command.is_some() && cli.signal.is_some() {
        return Err(anyhow!("`-s` cannot be used together with subcommands"));
    }

    if let Some(signal) = cli.signal {
        send_signal_from_pid_file(&pid_path_for_config(&cli.config), signal)?;
        return Ok(());
    }

    if let Some(command) = cli.command.as_ref()
        && admin_cli::run_admin_command(&cli.config, command)?
    {
        return Ok(());
    }

    rginx_observability::init_logging()
        .map_err(|error| anyhow!("failed to initialize logging: {error}"))?;

    let config = rginx_config::load_and_compile(&cli.config)
        .with_context(|| format!("failed to load {}", cli.config.display()))?;

    match cli.command {
        Some(Command::Check) if !cli.test_config => run_check_only(&cli.config, &config),
        _ if cli.test_config => run_check_only(&cli.config, &config),
        None => {
            let _pid_file = PidFileGuard::create(pid_path_for_config(&cli.config))
                .context("failed to write pid file")?;
            let runtime = build_runtime(config.runtime.worker_threads)
                .context("failed to construct tokio runtime")?;
            runtime
                .block_on(rginx_runtime::run(cli.config, config))
                .context("runtime exited with an error")
        }
        Some(Command::Check) => unreachable!("`check` subcommand and `-t` conflict at clap level"),
        Some(
            Command::Status
            | Command::Cache
            | Command::PurgeCache(_)
            | Command::SnapshotVersion
            | Command::Snapshot(_)
            | Command::Delta(_)
            | Command::Wait(_)
            | Command::Counters
            | Command::Traffic(_)
            | Command::Peers
            | Command::Upstreams(_),
        ) => {
            unreachable!("admin subcommands return before runtime initialization")
        }
    }
}

fn run_check_only(
    config_path: &std::path::Path,
    config: &rginx_config::ConfigSnapshot,
) -> anyhow::Result<()> {
    rginx_http::SharedState::from_config(config.clone())
        .context("failed to initialize runtime dependencies")?;

    print_check_success(config_path, build_check_summary(config));
    Ok(())
}
