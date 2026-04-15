mod admin_cli;
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
        Some(Command::Check) if !cli.test_config => {
            rginx_http::SharedState::from_config(config.clone())
                .context("failed to initialize runtime dependencies")?;

            print_check_success(&cli.config, build_check_summary(&config));
            Ok(())
        }
        _ if cli.test_config => {
            rginx_http::SharedState::from_config(config.clone())
                .context("failed to initialize runtime dependencies")?;

            print_check_success(&cli.config, build_check_summary(&config));
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
            | Command::Upstreams(_),
        ) => {
            unreachable!("admin subcommands return before runtime initialization")
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

struct CheckSummary {
    listener_model: &'static str,
    listener_count: usize,
    listener_binding_count: usize,
    listeners: Vec<CheckListenerSummary>,
    tls_enabled: bool,
    http3_enabled: bool,
    http3_early_data_enabled_listeners: usize,
    total_vhost_count: usize,
    total_route_count: usize,
    upstream_count: usize,
    worker_threads: Option<usize>,
    accept_workers: usize,
    route_transport: RouteTransportCheckDetails,
    tls: TlsCheckDetails,
}

struct CheckListenerSummary {
    id: String,
    name: String,
    listen_addr: std::net::SocketAddr,
    binding_count: usize,
    http3_enabled: bool,
    tls_enabled: bool,
    proxy_protocol_enabled: bool,
    default_certificate: Option<String>,
    keep_alive: bool,
    max_connections: Option<usize>,
    access_log_format_configured: bool,
    bindings: Vec<CheckListenerBindingSummary>,
}

struct CheckListenerBindingSummary {
    binding_name: String,
    transport: String,
    listen_addr: std::net::SocketAddr,
    protocols: Vec<String>,
    worker_count: usize,
    reuse_port_enabled: Option<bool>,
    advertise_alt_svc: Option<bool>,
    alt_svc_max_age_secs: Option<u64>,
    http3_max_concurrent_streams: Option<usize>,
    http3_stream_buffer_size: Option<usize>,
    http3_active_connection_id_limit: Option<u32>,
    http3_retry: Option<bool>,
    http3_host_key_path: Option<PathBuf>,
    http3_gso: Option<bool>,
    http3_early_data_enabled: Option<bool>,
}

struct RouteTransportCheckDetails {
    request_buffering_auto_routes: usize,
    request_buffering_on_routes: usize,
    request_buffering_off_routes: usize,
    response_buffering_auto_routes: usize,
    response_buffering_on_routes: usize,
    response_buffering_off_routes: usize,
    compression_auto_routes: usize,
    compression_off_routes: usize,
    compression_force_routes: usize,
    custom_compression_min_bytes_routes: usize,
    custom_compression_content_types_routes: usize,
    streaming_response_idle_timeout_routes: usize,
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
    listeners: Vec<rginx_http::TlsListenerStatusSnapshot>,
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
    let listen_addrs = if summary.listeners.is_empty() {
        "-".to_string()
    } else {
        summary
            .listeners
            .iter()
            .map(|listener| listener.listen_addr.to_string())
            .collect::<Vec<_>>()
            .join(",")
    };
    let bind_addrs = if summary.listeners.is_empty() {
        "-".to_string()
    } else {
        summary
            .listeners
            .iter()
            .flat_map(|listener| {
                listener
                    .bindings
                    .iter()
                    .map(|binding| format!("{}://{}", binding.transport, binding.listen_addr))
            })
            .collect::<Vec<_>>()
            .join(",")
    };
    println!(
        "configuration is valid: config={} listener_model={} listeners={} listener_bindings={} listen_addrs={} bind_addrs={} tls={} http3={} http3_early_data_enabled_listeners={} vhosts={} routes={} upstreams={} worker_threads={} accept_workers={}",
        config_path.display(),
        summary.listener_model,
        summary.listener_count,
        summary.listener_binding_count,
        listen_addrs,
        bind_addrs,
        if summary.tls_enabled { "enabled" } else { "disabled" },
        if summary.http3_enabled { "enabled" } else { "disabled" },
        summary.http3_early_data_enabled_listeners,
        summary.total_vhost_count,
        summary.total_route_count,
        summary.upstream_count,
        summary
            .worker_threads
            .map(|count: usize| count.to_string())
            .unwrap_or_else(|| "auto".to_string()),
        summary.accept_workers,
    );
    for listener in &summary.listeners {
        println!(
            "check_listener id={} name={} listen={} transport_bindings={} tls={} http3={} proxy_protocol={} default_certificate={} keep_alive={} max_connections={} access_log_format_configured={}",
            listener.id,
            listener.name,
            listener.listen_addr,
            listener.binding_count,
            if listener.tls_enabled { "enabled" } else { "disabled" },
            if listener.http3_enabled { "enabled" } else { "disabled" },
            listener.proxy_protocol_enabled,
            listener.default_certificate.as_deref().unwrap_or("-"),
            listener.keep_alive,
            listener
                .max_connections
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            listener.access_log_format_configured,
        );
        for binding in &listener.bindings {
            println!(
                "check_listener_binding listener={} binding={} transport={} listen={} protocols={} worker_count={} reuse_port_enabled={} advertise_alt_svc={} alt_svc_max_age_secs={} http3_max_concurrent_streams={} http3_stream_buffer_size={} http3_active_connection_id_limit={} http3_retry={} http3_host_key_path={} http3_gso={} http3_early_data_enabled={}",
                listener.id,
                binding.binding_name,
                binding.transport,
                binding.listen_addr,
                if binding.protocols.is_empty() {
                    "-".to_string()
                } else {
                    binding.protocols.join(",")
                },
                binding.worker_count,
                binding
                    .reuse_port_enabled
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                binding
                    .advertise_alt_svc
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                binding
                    .alt_svc_max_age_secs
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                binding
                    .http3_max_concurrent_streams
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                binding
                    .http3_stream_buffer_size
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                binding
                    .http3_active_connection_id_limit
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                binding
                    .http3_retry
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                binding
                    .http3_host_key_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "-".to_string()),
                binding.http3_gso.map(|value| value.to_string()).unwrap_or_else(|| "-".to_string()),
                binding
                    .http3_early_data_enabled
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string()),
            );
        }
    }
    println!(
        "route_transport_details=request_buffering_auto={} request_buffering_on={} request_buffering_off={} response_buffering_auto={} response_buffering_on={} response_buffering_off={} compression_auto={} compression_off={} compression_force={} custom_compression_min_bytes_routes={} custom_compression_content_types_routes={} streaming_response_idle_timeout_routes={}",
        summary.route_transport.request_buffering_auto_routes,
        summary.route_transport.request_buffering_on_routes,
        summary.route_transport.request_buffering_off_routes,
        summary.route_transport.response_buffering_auto_routes,
        summary.route_transport.response_buffering_on_routes,
        summary.route_transport.response_buffering_off_routes,
        summary.route_transport.compression_auto_routes,
        summary.route_transport.compression_off_routes,
        summary.route_transport.compression_force_routes,
        summary.route_transport.custom_compression_min_bytes_routes,
        summary.route_transport.custom_compression_content_types_routes,
        summary.route_transport.streaming_response_idle_timeout_routes,
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
    for listener in &summary.tls.listeners {
        println!(
            "tls_listener listener={} listener_id={} listen={} tls={} default_certificate={} tcp_versions={} tcp_alpn_protocols={} http3_enabled={} http3_listen={} http3_versions={} http3_alpn_protocols={} http3_max_concurrent_streams={} http3_stream_buffer_size={} http3_active_connection_id_limit={} http3_retry={} http3_host_key_path={} http3_gso={} http3_early_data_enabled={} sni_names={}",
            listener.listener_name,
            listener.listener_id,
            listener.listen_addr,
            listener.tls_enabled,
            listener.default_certificate.as_deref().unwrap_or("-"),
            listener
                .versions
                .as_ref()
                .filter(|versions| !versions.is_empty())
                .map(|versions| versions.join(","))
                .unwrap_or_else(|| "-".to_string()),
            if listener.alpn_protocols.is_empty() {
                "-".to_string()
            } else {
                listener.alpn_protocols.join(",")
            },
            listener.http3_enabled,
            listener
                .http3_listen_addr
                .map(|listen_addr| listen_addr.to_string())
                .unwrap_or_else(|| "-".to_string()),
            if listener.http3_versions.is_empty() {
                "-".to_string()
            } else {
                listener.http3_versions.join(",")
            },
            if listener.http3_alpn_protocols.is_empty() {
                "-".to_string()
            } else {
                listener.http3_alpn_protocols.join(",")
            },
            listener
                .http3_max_concurrent_streams
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            listener
                .http3_stream_buffer_size
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            listener
                .http3_active_connection_id_limit
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            listener.http3_retry.map(|value| value.to_string()).unwrap_or_else(|| "-".to_string()),
            listener
                .http3_host_key_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
            listener.http3_gso.map(|value| value.to_string()).unwrap_or_else(|| "-".to_string()),
            listener
                .http3_early_data_enabled
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            if listener.sni_names.is_empty() {
                "-".to_string()
            } else {
                listener.sni_names.join(",")
            },
        );
    }
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
            "tls_ocsp scope={} cert_path={} staple_path={} responder_urls={} nonce_mode={} responder_policy={} cache_loaded={} cache_size_bytes={} cache_modified_unix_ms={} auto_refresh_enabled={} last_refresh_unix_ms={} refreshes_total={} failures_total={} last_error={}",
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
            ocsp.nonce_mode,
            ocsp.responder_policy,
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

fn build_check_summary(config: &rginx_config::ConfigSnapshot) -> CheckSummary {
    CheckSummary {
        listener_model: listener_model(
            config.total_listener_count(),
            config.listeners.first().map(|listener| listener.id.as_str()),
            config.listeners.first().map(|listener| listener.name.as_str()),
        ),
        listener_count: config.total_listener_count(),
        listener_binding_count: config.total_listener_binding_count(),
        listeners: check_listener_summaries(config),
        tls_enabled: config.tls_enabled(),
        http3_enabled: config.http3_enabled(),
        http3_early_data_enabled_listeners: config
            .listeners
            .iter()
            .filter(|listener| {
                listener.http3.as_ref().is_some_and(|http3| http3.early_data_enabled)
            })
            .count(),
        total_vhost_count: config.total_vhost_count(),
        total_route_count: config.total_route_count(),
        upstream_count: config.upstreams.len(),
        worker_threads: config.runtime.worker_threads,
        accept_workers: config.runtime.accept_workers,
        route_transport: route_transport_check_details(config),
        tls: tls_check_details(config),
    }
}

fn route_transport_check_details(
    config: &rginx_config::ConfigSnapshot,
) -> RouteTransportCheckDetails {
    let mut details = RouteTransportCheckDetails {
        request_buffering_auto_routes: 0,
        request_buffering_on_routes: 0,
        request_buffering_off_routes: 0,
        response_buffering_auto_routes: 0,
        response_buffering_on_routes: 0,
        response_buffering_off_routes: 0,
        compression_auto_routes: 0,
        compression_off_routes: 0,
        compression_force_routes: 0,
        custom_compression_min_bytes_routes: 0,
        custom_compression_content_types_routes: 0,
        streaming_response_idle_timeout_routes: 0,
    };

    for route in all_routes(config) {
        match route.request_buffering {
            rginx_core::RouteBufferingPolicy::Auto => details.request_buffering_auto_routes += 1,
            rginx_core::RouteBufferingPolicy::On => details.request_buffering_on_routes += 1,
            rginx_core::RouteBufferingPolicy::Off => details.request_buffering_off_routes += 1,
        }

        match route.response_buffering {
            rginx_core::RouteBufferingPolicy::Auto => details.response_buffering_auto_routes += 1,
            rginx_core::RouteBufferingPolicy::On => details.response_buffering_on_routes += 1,
            rginx_core::RouteBufferingPolicy::Off => details.response_buffering_off_routes += 1,
        }

        match route.compression {
            rginx_core::RouteCompressionPolicy::Auto => details.compression_auto_routes += 1,
            rginx_core::RouteCompressionPolicy::Off => details.compression_off_routes += 1,
            rginx_core::RouteCompressionPolicy::Force => details.compression_force_routes += 1,
        }

        if route.compression_min_bytes.is_some() {
            details.custom_compression_min_bytes_routes += 1;
        }
        if !route.compression_content_types.is_empty() {
            details.custom_compression_content_types_routes += 1;
        }
        if route.streaming_response_idle_timeout.is_some() {
            details.streaming_response_idle_timeout_routes += 1;
        }
    }

    details
}

fn all_routes(config: &rginx_config::ConfigSnapshot) -> impl Iterator<Item = &rginx_core::Route> {
    std::iter::once(&config.default_vhost)
        .chain(config.vhosts.iter())
        .flat_map(|vhost| vhost.routes.iter())
}

fn check_listener_summaries(config: &rginx_config::ConfigSnapshot) -> Vec<CheckListenerSummary> {
    config
        .listeners
        .iter()
        .map(|listener| {
            let bindings = listener
                .transport_bindings()
                .into_iter()
                .map(|binding| CheckListenerBindingSummary {
                    binding_name: binding.name.to_string(),
                    transport: binding.kind.as_str().to_string(),
                    listen_addr: binding.listen_addr,
                    protocols: binding
                        .protocols
                        .into_iter()
                        .map(|protocol| protocol.as_str().to_string())
                        .collect(),
                    worker_count: config.runtime.accept_workers,
                    reuse_port_enabled: (binding.kind == rginx_core::ListenerTransportKind::Udp)
                        .then_some(config.runtime.accept_workers > 1),
                    advertise_alt_svc: binding.alt_svc_max_age.map(|_| binding.advertise_alt_svc),
                    alt_svc_max_age_secs: binding.alt_svc_max_age.map(|max_age| max_age.as_secs()),
                    http3_max_concurrent_streams: binding.http3_max_concurrent_streams,
                    http3_stream_buffer_size: binding.http3_stream_buffer_size,
                    http3_active_connection_id_limit: binding.http3_active_connection_id_limit,
                    http3_retry: binding.http3_retry,
                    http3_host_key_path: binding.http3_host_key_path.clone(),
                    http3_gso: binding.http3_gso,
                    http3_early_data_enabled: binding.http3_early_data_enabled,
                })
                .collect::<Vec<_>>();

            CheckListenerSummary {
                id: listener.id.clone(),
                name: listener.name.clone(),
                listen_addr: listener.server.listen_addr,
                binding_count: listener.binding_count(),
                http3_enabled: listener.http3_enabled(),
                tls_enabled: listener.tls_enabled(),
                proxy_protocol_enabled: listener.proxy_protocol_enabled,
                default_certificate: listener.server.default_certificate.clone(),
                keep_alive: listener.server.keep_alive,
                max_connections: listener.server.max_connections,
                access_log_format_configured: listener.server.access_log_format.is_some(),
                bindings,
            }
        })
        .collect()
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
    let (sni_bindings, sni_conflicts, default_certificate_bindings) = (
        tls.sni_bindings
            .iter()
            .map(|binding| TlsSniBindingCheck {
                listener_name: binding.listener_name.clone(),
                server_name: binding.server_name.clone(),
                fingerprints: binding.fingerprints.clone(),
                scopes: binding.certificate_scopes.clone(),
                default_selected: binding.default_selected,
            })
            .collect(),
        tls.sni_conflicts
            .iter()
            .map(|binding| TlsSniBindingCheck {
                listener_name: binding.listener_name.clone(),
                server_name: binding.server_name.clone(),
                fingerprints: binding.fingerprints.clone(),
                scopes: binding.certificate_scopes.clone(),
                default_selected: binding.default_selected,
            })
            .collect(),
        tls.default_certificate_bindings
            .iter()
            .map(|binding| TlsDefaultCertificateBindingCheck {
                listener_name: binding.listener_name.clone(),
                server_name: binding.server_name.clone(),
                fingerprints: binding.fingerprints.clone(),
                scopes: binding.certificate_scopes.clone(),
            })
            .collect(),
    );

    TlsCheckDetails {
        listener_tls_profiles,
        vhost_tls_overrides,
        sni_name_count,
        certificate_bundle_count,
        default_certificates,
        expiring_certificates,
        reloadable_fields: tls.reload_boundary.reloadable_fields,
        restart_required_fields: tls.reload_boundary.restart_required_fields,
        listeners: tls.listeners,
        vhost_bindings: tls.vhost_bindings,
        ocsp: tls.ocsp,
        certificates: tls.certificates,
        sni_bindings,
        sni_conflicts,
        default_certificate_bindings,
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
