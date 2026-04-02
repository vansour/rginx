mod cli;

#[cfg(not(target_os = "linux"))]
compile_error!("rginx supports Linux only");

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use clap::Parser;

use crate::cli::{Cli, Command, SignalCommand, pid_path_for_config};

fn main() -> anyhow::Result<()> {
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

    rginx_observability::init_logging()
        .map_err(|error| anyhow!("failed to initialize logging: {error}"))?;

    let config = rginx_config::load_and_compile(&cli.config)
        .with_context(|| format!("failed to load {}", cli.config.display()))?;

    match cli.command {
        Some(Command::Check) if !cli.test_config => {
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
        _ if cli.test_config => {
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
            let _pid_file = PidFileGuard::create(pid_path_for_config(&cli.config))
                .context("failed to write pid file")?;
            let runtime = build_runtime(config.runtime.worker_threads)
                .context("failed to construct tokio runtime")?;
            runtime
                .block_on(rginx_runtime::run(cli.config, config))
                .context("runtime exited with an error")
        }
        Some(Command::Check) => unreachable!("`check` subcommand and `-t` conflict at clap level"),
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

struct PidFileGuard {
    path: PathBuf,
}

impl PidFileGuard {
    fn create(path: PathBuf) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create pid directory {}", parent.display()))?;
        }

        fs::write(&path, format!("{}\n", std::process::id()))
            .with_context(|| format!("failed to write pid file {}", path.display()))?;

        Ok(Self { path })
    }
}

impl Drop for PidFileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn send_signal_from_pid_file(pid_path: &Path, signal: SignalCommand) -> anyhow::Result<()> {
    let raw_pid = fs::read_to_string(pid_path)
        .with_context(|| format!("failed to read pid file {}", pid_path.display()))?;
    let pid = raw_pid
        .trim()
        .parse::<i32>()
        .with_context(|| format!("invalid pid file contents in {}", pid_path.display()))?;

    let signal_number = match signal {
        SignalCommand::Reload => libc::SIGHUP,
        SignalCommand::Stop => libc::SIGTERM,
        SignalCommand::Quit => libc::SIGQUIT,
    };

    let result = unsafe { libc::kill(pid, signal_number) };
    if result != 0 {
        return Err(anyhow!(
            "failed to send signal `{}` to pid {} from {}: {}",
            signal_name(signal),
            pid,
            pid_path.display(),
            std::io::Error::last_os_error()
        ));
    }

    println!("signal `{}` sent to pid {} via {}", signal_name(signal), pid, pid_path.display());
    Ok(())
}

fn signal_name(signal: SignalCommand) -> &'static str {
    match signal {
        SignalCommand::Reload => "reload",
        SignalCommand::Stop => "stop",
        SignalCommand::Quit => "quit",
    }
}
