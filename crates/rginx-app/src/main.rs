mod admin_cli;
mod cli;
mod migrate_nginx;

#[cfg(not(target_os = "linux"))]
compile_error!("rginx supports Linux only");

use std::collections::BTreeMap;
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
                    tls: tls_check_details(&config),
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
                    tls: tls_check_details(&config),
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
    tls: TlsCheckDetails,
}

struct TlsCheckDetails {
    listener_tls_profiles: usize,
    vhost_tls_overrides: usize,
    sni_name_count: usize,
    certificate_bundle_count: usize,
    default_certificates: Vec<String>,
    expiring_certificates: Vec<String>,
    reloadable_fields: Vec<String>,
    restart_required_fields: Vec<String>,
    certificates: Vec<rginx_http::TlsCertificateStatusSnapshot>,
    ocsp: Vec<rginx_http::TlsOcspStatusSnapshot>,
    vhost_bindings: Vec<rginx_http::TlsVhostBindingSnapshot>,
    sni_bindings: Vec<TlsSniBindingCheck>,
    sni_conflicts: Vec<TlsSniBindingCheck>,
    default_certificate_bindings: Vec<TlsDefaultCertificateBindingCheck>,
}

struct TlsSniBindingCheck {
    listener_name: String,
    server_name: String,
    fingerprints: Vec<String>,
    scopes: Vec<String>,
    default_selected: bool,
}

struct TlsDefaultCertificateBindingCheck {
    listener_name: String,
    server_name: String,
    fingerprints: Vec<String>,
    scopes: Vec<String>,
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
    println!("reload_requires_restart_for={}", summary.tls.restart_required_fields.join(","));
    println!(
        "tls_details=listener_profiles={} vhost_overrides={} sni_names={} certificate_bundles={}",
        summary.tls.listener_tls_profiles,
        summary.tls.vhost_tls_overrides,
        summary.tls.sni_name_count,
        summary.tls.certificate_bundle_count,
    );
    println!("reload_tls_updates={}", summary.tls.reloadable_fields.join(","));
    if !summary.tls.default_certificates.is_empty() {
        println!("tls_default_certificates={}", summary.tls.default_certificates.join(","));
    }
    if summary.tls.expiring_certificates.is_empty() {
        println!("tls_expiring_certificates=-");
    } else {
        println!("tls_expiring_certificates={}", summary.tls.expiring_certificates.join(","));
    }
    println!("tls_restart_required_fields={}", summary.tls.restart_required_fields.join(","));
    for certificate in &summary.tls.certificates {
        println!(
            "tls_certificate scope={} sha256={} subject={:?} issuer={:?} serial={:?} chain_length={} diagnostics={} cert_path={}",
            certificate.scope,
            certificate.fingerprint_sha256.as_deref().unwrap_or("-"),
            certificate.subject,
            certificate.issuer,
            certificate.serial_number,
            certificate.chain_length,
            if certificate.chain_diagnostics.is_empty() {
                "-".to_string()
            } else {
                certificate.chain_diagnostics.join("|")
            },
            certificate.cert_path.display(),
        );
    }
    for ocsp in &summary.tls.ocsp {
        println!(
            "tls_ocsp scope={} cert_path={} staple_path={} responder_urls={} cache_loaded={} cache_size_bytes={} cache_modified_unix_ms={} auto_refresh_enabled={} last_refresh_unix_ms={} refreshes_total={} failures_total={} last_error={}",
            ocsp.scope,
            ocsp.cert_path.display(),
            ocsp.ocsp_staple_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
            if ocsp.responder_urls.is_empty() {
                "-".to_string()
            } else {
                ocsp.responder_urls.join(",")
            },
            ocsp.cache_loaded,
            ocsp.cache_size_bytes.map(|value| value.to_string()).unwrap_or_else(|| "-".to_string()),
            ocsp.cache_modified_unix_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            ocsp.auto_refresh_enabled,
            ocsp.last_refresh_unix_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            ocsp.refreshes_total,
            ocsp.failures_total,
            ocsp.last_error.as_deref().unwrap_or("-"),
        );
    }
    for binding in &summary.tls.vhost_bindings {
        println!(
            "tls_vhost_binding listener={} vhost={} server_names={} certificate_scopes={} fingerprints={} default_selected={}",
            binding.listener_name,
            binding.vhost_id,
            if binding.server_names.is_empty() {
                "-".to_string()
            } else {
                binding.server_names.join(",")
            },
            if binding.certificate_scopes.is_empty() {
                "-".to_string()
            } else {
                binding.certificate_scopes.join(",")
            },
            if binding.fingerprints.is_empty() {
                "-".to_string()
            } else {
                binding.fingerprints.join(",")
            },
            binding.default_selected,
        );
    }
    for binding in &summary.tls.sni_bindings {
        let fingerprints = if binding.fingerprints.is_empty() {
            "-".to_string()
        } else {
            binding.fingerprints.join(",")
        };
        let scopes =
            if binding.scopes.is_empty() { "-".to_string() } else { binding.scopes.join(",") };
        println!(
            "tls_sni_binding listener={} server_name={} fingerprints={} scopes={} default_selected={}",
            binding.listener_name,
            binding.server_name,
            fingerprints,
            scopes,
            binding.default_selected,
        );
    }
    if summary.tls.sni_conflicts.is_empty() {
        println!("tls_sni_conflicts=-");
    } else {
        for binding in &summary.tls.sni_conflicts {
            println!(
                "tls_sni_conflict listener={} server_name={} fingerprints={} scopes={}",
                binding.listener_name,
                binding.server_name,
                binding.fingerprints.join(","),
                binding.scopes.join(","),
            );
        }
    }
    for binding in &summary.tls.default_certificate_bindings {
        let fingerprints = if binding.fingerprints.is_empty() {
            "-".to_string()
        } else {
            binding.fingerprints.join(",")
        };
        let scopes =
            if binding.scopes.is_empty() { "-".to_string() } else { binding.scopes.join(",") };
        println!(
            "tls_default_certificate_binding listener={} server_name={} fingerprints={} scopes={}",
            binding.listener_name, binding.server_name, fingerprints, scopes,
        );
    }
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

fn tls_check_details(config: &rginx_config::ConfigSnapshot) -> TlsCheckDetails {
    let tls = rginx_http::tls_runtime_snapshot_for_config(config);
    let listener_tls_profiles =
        config.listeners.iter().filter(|listener| listener.server.tls.is_some()).count();
    let vhost_tls_overrides = std::iter::once(&config.default_vhost)
        .chain(config.vhosts.iter())
        .filter(|vhost| vhost.tls.is_some())
        .count();
    let sni_name_count = config
        .listeners
        .iter()
        .filter(|listener| listener.server.tls.is_some())
        .map(|_| config.default_vhost.server_names.len())
        .sum::<usize>()
        + config
            .vhosts
            .iter()
            .filter(|vhost| vhost.tls.is_some())
            .map(|vhost| vhost.server_names.len())
            .sum::<usize>();
    let certificate_bundle_count = config
        .listeners
        .iter()
        .filter_map(|listener| listener.server.tls.as_ref())
        .map(|tls| 1 + tls.additional_certificates.len())
        .sum::<usize>()
        + std::iter::once(&config.default_vhost)
            .chain(config.vhosts.iter())
            .filter_map(|vhost| vhost.tls.as_ref())
            .map(|tls| 1 + tls.additional_certificates.len())
            .sum::<usize>();
    let default_certificates = config
        .listeners
        .iter()
        .filter_map(|listener| {
            listener
                .server
                .default_certificate
                .as_ref()
                .map(|name| format!("{}={}", listener.name, name))
        })
        .collect();
    let expiring_certificates = tls
        .certificates
        .iter()
        .filter_map(|certificate| {
            certificate
                .expires_in_days
                .and_then(|days| (days <= 30).then(|| format!("{}:{}d", certificate.scope, days)))
        })
        .collect();
    let (sni_bindings, sni_conflicts, default_certificate_bindings) =
        tls_sni_diagnostics(config, &tls.certificates);

    TlsCheckDetails {
        listener_tls_profiles,
        vhost_tls_overrides,
        sni_name_count,
        certificate_bundle_count,
        default_certificates,
        expiring_certificates,
        reloadable_fields: tls.reload_boundary.reloadable_fields,
        restart_required_fields: tls.reload_boundary.restart_required_fields,
        vhost_bindings: tls.vhost_bindings,
        ocsp: tls.ocsp,
        certificates: tls.certificates,
        sni_bindings,
        sni_conflicts,
        default_certificate_bindings,
    }
}

fn tls_sni_diagnostics(
    config: &rginx_config::ConfigSnapshot,
    certificates: &[rginx_http::TlsCertificateStatusSnapshot],
) -> (Vec<TlsSniBindingCheck>, Vec<TlsSniBindingCheck>, Vec<TlsDefaultCertificateBindingCheck>) {
    let fingerprint_by_scope = certificates
        .iter()
        .map(|certificate| {
            (
                certificate.scope.clone(),
                certificate.fingerprint_sha256.clone().unwrap_or_else(|| "-".to_string()),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let mut bindings = BTreeMap::<(String, String), TlsSniBindingCheck>::new();
    for listener in &config.listeners {
        if !listener.tls_enabled() {
            continue;
        }

        if listener.server.tls.is_some() {
            let scope = format!("listener:{}", listener.name);
            let fingerprint =
                fingerprint_by_scope.get(&scope).cloned().unwrap_or_else(|| "-".to_string());
            for server_name in &config.default_vhost.server_names {
                let binding = bindings
                    .entry((listener.name.clone(), server_name.clone()))
                    .or_insert_with(|| TlsSniBindingCheck {
                        listener_name: listener.name.clone(),
                        server_name: server_name.clone(),
                        fingerprints: Vec::new(),
                        scopes: Vec::new(),
                        default_selected: false,
                    });
                if !binding.fingerprints.iter().any(|value| value == &fingerprint) {
                    binding.fingerprints.push(fingerprint.clone());
                }
                if !binding.scopes.iter().any(|value| value == &scope) {
                    binding.scopes.push(scope.clone());
                }
            }
        }

        for vhost in &config.vhosts {
            if vhost.tls.is_none() {
                continue;
            }
            let scope = format!("vhost:{}", vhost.id);
            let fingerprint =
                fingerprint_by_scope.get(&scope).cloned().unwrap_or_else(|| "-".to_string());
            for server_name in &vhost.server_names {
                let binding = bindings
                    .entry((listener.name.clone(), server_name.clone()))
                    .or_insert_with(|| TlsSniBindingCheck {
                        listener_name: listener.name.clone(),
                        server_name: server_name.clone(),
                        fingerprints: Vec::new(),
                        scopes: Vec::new(),
                        default_selected: false,
                    });
                if !binding.fingerprints.iter().any(|value| value == &fingerprint) {
                    binding.fingerprints.push(fingerprint.clone());
                }
                if !binding.scopes.iter().any(|value| value == &scope) {
                    binding.scopes.push(scope.clone());
                }
            }
        }
    }

    let mut default_certificate_bindings = Vec::new();
    for listener in &config.listeners {
        let Some(default_certificate) = listener.server.default_certificate.as_ref() else {
            continue;
        };
        if let Some(binding) =
            bindings.get_mut(&(listener.name.clone(), default_certificate.clone()))
        {
            binding.default_selected = true;
            default_certificate_bindings.push(TlsDefaultCertificateBindingCheck {
                listener_name: listener.name.clone(),
                server_name: default_certificate.clone(),
                fingerprints: binding.fingerprints.clone(),
                scopes: binding.scopes.clone(),
            });
        }
    }

    let mut sni_bindings = bindings.into_values().collect::<Vec<_>>();
    sni_bindings.sort_by(|left, right| {
        left.listener_name.cmp(&right.listener_name).then(left.server_name.cmp(&right.server_name))
    });
    let sni_conflicts = sni_bindings
        .iter()
        .filter(|binding| binding.fingerprints.len() > 1)
        .map(|binding| TlsSniBindingCheck {
            listener_name: binding.listener_name.clone(),
            server_name: binding.server_name.clone(),
            fingerprints: binding.fingerprints.clone(),
            scopes: binding.scopes.clone(),
            default_selected: binding.default_selected,
        })
        .collect();

    (sni_bindings, sni_conflicts, default_certificate_bindings)
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
