mod admin_cli;
mod cli;
mod migrate_nginx;

#[cfg(not(target_os = "linux"))]
compile_error!("rginx supports Linux only");

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use clap::Parser;

use crate::cli::{Cli, Command, MigrateNginxArgs, SignalCommand, pid_path_for_config};

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

    if let Some(command) = cli.command.as_ref() {
        if admin_cli::run_admin_command(&cli.config, command)? {
            return Ok(());
        }

        if let Command::MigrateNginx(args) = command {
            run_migrate_nginx(args)?;
            return Ok(());
        }
    }

    rginx_observability::init_logging()
        .map_err(|error| anyhow!("failed to initialize logging: {error}"))?;

    let config = rginx_config::load_and_compile(&cli.config)
        .with_context(|| format!("failed to load {}", cli.config.display()))?;

    match cli.command {
        Some(Command::Check) if !cli.test_config => {
            rginx_http::SharedState::from_config(config.clone())
                .context("failed to initialize runtime dependencies")?;

            print_check_success(
                &cli.config,
                CheckSummary {
                    listener_model: listener_model(
                        config.total_listener_count(),
                        config.listeners.first().map(|listener| listener.id.as_str()),
                        config.listeners.first().map(|listener| listener.name.as_str()),
                    ),
                    listener_count: config.total_listener_count(),
                    listen_addr: config.server.listen_addr,
                    tls_enabled: config.tls_enabled(),
                    total_vhost_count: config.total_vhost_count(),
                    total_route_count: config.total_route_count(),
                    upstream_count: config.upstreams.len(),
                    worker_threads: config.runtime.worker_threads,
                    accept_workers: config.runtime.accept_workers,
                },
            );
            Ok(())
        }
        _ if cli.test_config => {
            rginx_http::SharedState::from_config(config.clone())
                .context("failed to initialize runtime dependencies")?;

            print_check_success(
                &cli.config,
                CheckSummary {
                    listener_model: listener_model(
                        config.total_listener_count(),
                        config.listeners.first().map(|listener| listener.id.as_str()),
                        config.listeners.first().map(|listener| listener.name.as_str()),
                    ),
                    listener_count: config.total_listener_count(),
                    listen_addr: config.server.listen_addr,
                    tls_enabled: config.tls_enabled(),
                    total_vhost_count: config.total_vhost_count(),
                    total_route_count: config.total_route_count(),
                    upstream_count: config.upstreams.len(),
                    worker_threads: config.runtime.worker_threads,
                    accept_workers: config.runtime.accept_workers,
                },
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
        Some(
            Command::Status
            | Command::SnapshotVersion
            | Command::Snapshot(_)
            | Command::Delta(_)
            | Command::Wait(_)
            | Command::Counters
            | Command::Traffic(_)
            | Command::Peers
            | Command::Upstreams(_)
            | Command::MigrateNginx(_),
        ) => {
            unreachable!("admin subcommands return before runtime initialization")
        }
    }
}

fn run_migrate_nginx(args: &MigrateNginxArgs) -> anyhow::Result<()> {
    let migrated = migrate_nginx::migrate_file(&args.input)?;

    if let Some(output) = &args.output {
        fs::write(output, &migrated.ron)
            .with_context(|| format!("failed to write migrated config {}", output.display()))?;
        eprintln!(
            "wrote migrated rginx config to {} (warnings: {})",
            output.display(),
            migrated.warnings.len()
        );
    } else {
        print!("{}", migrated.ron);
    }

    if !migrated.warnings.is_empty() {
        eprintln!("migration warnings:");
        for warning in &migrated.warnings {
            eprintln!("- {warning}");
        }
    }

    Ok(())
}

fn build_runtime(worker_threads: Option<usize>) -> anyhow::Result<tokio::runtime::Runtime> {
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    if let Some(worker_threads) = worker_threads {
        builder.worker_threads(worker_threads);
    }
    builder.build().map_err(|error| anyhow!("failed to build tokio runtime: {error}"))
}

struct CheckSummary {
    listener_model: &'static str,
    listener_count: usize,
    listen_addr: std::net::SocketAddr,
    tls_enabled: bool,
    total_vhost_count: usize,
    total_route_count: usize,
    upstream_count: usize,
    worker_threads: Option<usize>,
    accept_workers: usize,
}

fn print_check_success(config_path: &Path, summary: CheckSummary) {
    println!(
        "configuration is valid: config={} listener_model={} listeners={} listen={} tls={} vhosts={} routes={} upstreams={} worker_threads={} accept_workers={}",
        config_path.display(),
        summary.listener_model,
        summary.listener_count,
        summary.listen_addr,
        if summary.tls_enabled { "enabled" } else { "disabled" },
        summary.total_vhost_count,
        summary.total_route_count,
        summary.upstream_count,
        summary
            .worker_threads
            .map(|count: usize| count.to_string())
            .unwrap_or_else(|| "auto".to_string()),
        summary.accept_workers,
    );
    println!(
        "reload_requires_restart_for=listen,listeners,runtime.worker_threads,runtime.accept_workers"
    );
}

fn listener_model(
    listener_count: usize,
    first_listener_id: Option<&str>,
    first_listener_name: Option<&str>,
) -> &'static str {
    if listener_count == 1
        && first_listener_id == Some("default")
        && first_listener_name == Some("default")
    {
        "legacy"
    } else {
        "explicit"
    }
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
        let Ok(current) = fs::read_to_string(&self.path) else {
            return;
        };
        if current.trim() == std::process::id().to_string() {
            let _ = fs::remove_file(&self.path);
        }
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
        SignalCommand::Restart => libc::SIGUSR2,
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
        SignalCommand::Restart => "restart",
        SignalCommand::Stop => "stop",
        SignalCommand::Quit => "quit",
    }
}
