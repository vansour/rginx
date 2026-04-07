mod cli;
mod migrate_nginx;

#[cfg(not(target_os = "linux"))]
compile_error!("rginx supports Linux only");

use std::fs;
use std::io::{BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, anyhow};
use clap::Parser;
use rginx_runtime::admin::{
    AdminRequest, AdminResponse, RevisionSnapshot, admin_socket_path_for_config,
};

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
        match command {
            Command::Status => {
                print_admin_status(&cli.config)?;
                return Ok(());
            }
            Command::Counters => {
                print_admin_counters(&cli.config)?;
                return Ok(());
            }
            Command::Peers => {
                print_admin_peers(&cli.config)?;
                return Ok(());
            }
            Command::MigrateNginx(args) => {
                run_migrate_nginx(args)?;
                return Ok(());
            }
            Command::Check => {}
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
        Some(Command::Status | Command::Counters | Command::Peers | Command::MigrateNginx(_)) => {
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

fn print_admin_status(config_path: &Path) -> anyhow::Result<()> {
    match query_admin_socket(config_path, AdminRequest::GetStatus)? {
        AdminResponse::Status(status) => {
            println!("revision={}", status.revision);
            println!(
                "config_path={}",
                status
                    .config_path
                    .as_deref()
                    .map(Path::display)
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string())
            );
            println!("listen={}", status.listen_addr);
            println!(
                "worker_threads={}",
                status
                    .worker_threads
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "auto".to_string())
            );
            println!("accept_workers={}", status.accept_workers);
            println!("vhosts={}", status.total_vhosts);
            println!("routes={}", status.total_routes);
            println!("upstreams={}", status.total_upstreams);
            println!("tls={}", if status.tls_enabled { "enabled" } else { "disabled" });
            println!("active_connections={}", status.active_connections);
            println!("reload_attempts={}", status.reload.attempts_total);
            println!("reload_successes={}", status.reload.successes_total);
            println!("reload_failures={}", status.reload.failures_total);
            println!("last_reload={}", render_last_reload(status.reload.last_result.as_ref()));
            Ok(())
        }
        response => Err(unexpected_admin_response("status", &response)),
    }
}

fn print_admin_counters(config_path: &Path) -> anyhow::Result<()> {
    match query_admin_socket(config_path, AdminRequest::GetCounters)? {
        AdminResponse::Counters(counters) => {
            println!(
                "downstream_connections_accepted_total={}",
                counters.downstream_connections_accepted
            );
            println!(
                "downstream_connections_rejected_total={}",
                counters.downstream_connections_rejected
            );
            println!("downstream_requests_total={}", counters.downstream_requests);
            println!("downstream_responses_total={}", counters.downstream_responses);
            println!("downstream_responses_1xx_total={}", counters.downstream_responses_1xx);
            println!("downstream_responses_2xx_total={}", counters.downstream_responses_2xx);
            println!("downstream_responses_3xx_total={}", counters.downstream_responses_3xx);
            println!("downstream_responses_4xx_total={}", counters.downstream_responses_4xx);
            println!("downstream_responses_5xx_total={}", counters.downstream_responses_5xx);
            Ok(())
        }
        response => Err(unexpected_admin_response("counters", &response)),
    }
}

fn print_admin_peers(config_path: &Path) -> anyhow::Result<()> {
    match query_admin_socket(config_path, AdminRequest::GetPeerHealth)? {
        AdminResponse::PeerHealth(upstreams) => {
            for upstream in upstreams {
                println!(
                    "upstream={} unhealthy_after_failures={} cooldown_ms={} active_health_enabled={}",
                    upstream.upstream_name,
                    upstream.unhealthy_after_failures,
                    upstream.cooldown_ms,
                    upstream.active_health_enabled
                );
                for peer in upstream.peers {
                    println!(
                        "  peer={} backup={} weight={} available={} passive_failures={} passive_cooldown_remaining_ms={} passive_pending_recovery={} active_unhealthy={} active_successes={} active_requests={}",
                        peer.peer_url,
                        peer.backup,
                        peer.weight,
                        peer.available,
                        peer.passive_consecutive_failures,
                        peer.passive_cooldown_remaining_ms
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "-".to_string()),
                        peer.passive_pending_recovery,
                        peer.active_unhealthy,
                        peer.active_consecutive_successes,
                        peer.active_requests,
                    );
                }
            }
            Ok(())
        }
        response => Err(unexpected_admin_response("peers", &response)),
    }
}

fn query_admin_socket(config_path: &Path, request: AdminRequest) -> anyhow::Result<AdminResponse> {
    let socket_path = admin_socket_path_for_config(config_path);
    let mut stream = UnixStream::connect(&socket_path)
        .with_context(|| format!("failed to connect to admin socket {}", socket_path.display()))?;
    serde_json::to_writer(&mut stream, &request)
        .context("failed to encode admin socket request")?;
    stream.write_all(b"\n").context("failed to terminate admin socket request")?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .context("failed to shutdown admin socket write side")?;

    let mut response = String::new();
    BufReader::new(stream)
        .read_to_string(&mut response)
        .context("failed to read admin socket response")?;
    let response: AdminResponse =
        serde_json::from_str(response.trim()).context("failed to decode admin socket response")?;
    match response {
        AdminResponse::Error { message } => Err(anyhow!("admin socket error: {message}")),
        response => Ok(response),
    }
}

fn unexpected_admin_response(command: &str, response: &AdminResponse) -> anyhow::Error {
    anyhow!("unexpected admin response for `{command}`: {}", admin_response_kind(response))
}

fn admin_response_kind(response: &AdminResponse) -> &'static str {
    match response {
        AdminResponse::Status(_) => "status",
        AdminResponse::Counters(_) => "counters",
        AdminResponse::PeerHealth(_) => "peer_health",
        AdminResponse::Revision(RevisionSnapshot { .. }) => "revision",
        AdminResponse::Error { .. } => "error",
    }
}

fn render_last_reload(result: Option<&rginx_http::ReloadResultSnapshot>) -> String {
    let Some(result) = result else {
        return "-".to_string();
    };

    let finished_at = result
        .finished_at_unix_ms
        .checked_div(1000)
        .and_then(|seconds| UNIX_EPOCH.checked_add(std::time::Duration::from_secs(seconds)))
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|| result.finished_at_unix_ms.to_string());

    match &result.outcome {
        rginx_http::ReloadOutcomeSnapshot::Success { revision } => {
            format!("success revision={revision} finished_at_unix_s={finished_at}")
        }
        rginx_http::ReloadOutcomeSnapshot::Failure { error } => {
            format!("failure error={error:?} finished_at_unix_s={finished_at}")
        }
    }
}
