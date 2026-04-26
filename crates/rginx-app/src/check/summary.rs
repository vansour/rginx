use std::path::PathBuf;

use super::routes::{RouteTransportCheckDetails, route_transport_check_details};
use super::tls::{TlsCheckDetails, tls_check_details};

pub(crate) struct CheckSummary {
    pub(super) listener_model: &'static str,
    pub(super) listener_count: usize,
    pub(super) listener_binding_count: usize,
    pub(super) listeners: Vec<CheckListenerSummary>,
    pub(super) tls_enabled: bool,
    pub(super) http3_enabled: bool,
    pub(super) http3_early_data_enabled_listeners: usize,
    pub(super) total_vhost_count: usize,
    pub(super) total_route_count: usize,
    pub(super) upstream_count: usize,
    pub(super) worker_threads: Option<usize>,
    pub(super) accept_workers: usize,
    pub(super) route_transport: RouteTransportCheckDetails,
    pub(super) tls: TlsCheckDetails,
}

pub(super) struct CheckListenerSummary {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) listen_addr: std::net::SocketAddr,
    pub(super) binding_count: usize,
    pub(super) http3_enabled: bool,
    pub(super) tls_enabled: bool,
    pub(super) proxy_protocol_enabled: bool,
    pub(super) default_certificate: Option<String>,
    pub(super) keep_alive: bool,
    pub(super) max_connections: Option<usize>,
    pub(super) access_log_format_configured: bool,
    pub(super) bindings: Vec<CheckListenerBindingSummary>,
}

pub(super) struct CheckListenerBindingSummary {
    pub(super) binding_name: String,
    pub(super) transport: String,
    pub(super) listen_addr: std::net::SocketAddr,
    pub(super) protocols: Vec<String>,
    pub(super) worker_count: usize,
    pub(super) reuse_port_enabled: Option<bool>,
    pub(super) advertise_alt_svc: Option<bool>,
    pub(super) alt_svc_max_age_secs: Option<u64>,
    pub(super) http3_max_concurrent_streams: Option<usize>,
    pub(super) http3_stream_buffer_size: Option<usize>,
    pub(super) http3_active_connection_id_limit: Option<u32>,
    pub(super) http3_retry: Option<bool>,
    pub(super) http3_host_key_path: Option<PathBuf>,
    pub(super) http3_gso: Option<bool>,
    pub(super) http3_early_data_enabled: Option<bool>,
}

pub(crate) fn build_check_summary(config: &rginx_config::ConfigSnapshot) -> CheckSummary {
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

fn check_listener_summaries(config: &rginx_config::ConfigSnapshot) -> Vec<CheckListenerSummary> {
    config
        .listeners
        .iter()
        .map(|listener| {
            let bindings = listener
                .transport_bindings()
                .into_iter()
                .map(|binding| CheckListenerBindingSummary {
                    advertise_alt_svc: (binding.kind == rginx_core::ListenerTransportKind::Udp)
                        .then_some(binding.advertise_alt_svc),
                    alt_svc_max_age_secs: if binding.kind == rginx_core::ListenerTransportKind::Udp
                    {
                        binding.alt_svc_max_age.map(|max_age| max_age.as_secs())
                    } else {
                        None
                    },
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
