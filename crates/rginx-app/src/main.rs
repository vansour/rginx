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
use crate::cli::{AcmeCommand, Cli, Command, pid_path_for_config};
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

    if let Some(Command::Acme(args)) = cli.command.as_ref() {
        return match &args.command {
            AcmeCommand::Issue(issue) if issue.once => run_acme_issue_once(&cli.config),
            _ => unreachable!("clap enforces the one-shot ACME command shape"),
        };
    }

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
        Some(Command::Acme(_)) => {
            unreachable!("ACME subcommands return before standard config initialization")
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

fn run_acme_issue_once(config_path: &std::path::Path) -> anyhow::Result<()> {
    let config = rginx_config::load_and_compile_for_acme_issue(config_path)
        .with_context(|| format!("failed to load {}", config_path.display()))?;
    let runtime = build_runtime(config.runtime.worker_threads)
        .context("failed to construct tokio runtime")?;
    let summary = runtime
        .block_on(rginx_runtime::acme::issue_once(&config))
        .context("one-shot ACME issuance failed")?;

    if summary.is_success() {
        println!(
            "acme issue completed: total={} issued={} skipped={}",
            summary.total, summary.issued, summary.skipped
        );
        return Ok(());
    }

    Err(anyhow!(
        "acme issue completed with failures: {}",
        summary
            .failures
            .iter()
            .map(|failure| format!("{} ({})", failure.scope, failure.error))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}
