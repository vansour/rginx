use crate::admin_cli::render::print_record;

pub(super) fn print_status_listeners(listeners: &[rginx_http::RuntimeListenerSnapshot]) {
    for listener in listeners {
        print_record(
            "status_listener",
            [
                ("listener", listener.listener_name.clone()),
                ("listener_id", listener.listener_id.clone()),
                ("listen", listener.listen_addr.to_string()),
                ("transport_bindings", listener.binding_count.to_string()),
                ("tls", if listener.tls_enabled { "enabled" } else { "disabled" }.to_string()),
                ("http3", if listener.http3_enabled { "enabled" } else { "disabled" }.to_string()),
                ("proxy_protocol", listener.proxy_protocol_enabled.to_string()),
                (
                    "default_certificate",
                    listener.default_certificate.clone().unwrap_or_else(|| "-".to_string()),
                ),
                ("keep_alive", listener.keep_alive.to_string()),
                (
                    "max_connections",
                    listener
                        .max_connections
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                ),
                ("access_log_format_configured", listener.access_log_format_configured.to_string()),
            ],
        );

        for binding in &listener.bindings {
            print_record(
                "status_listener_binding",
                [
                    ("listener", listener.listener_name.clone()),
                    ("listener_id", listener.listener_id.clone()),
                    ("binding", binding.binding_name.clone()),
                    ("transport", binding.transport.clone()),
                    ("listen", binding.listen_addr.to_string()),
                    (
                        "protocols",
                        if binding.protocols.is_empty() {
                            "-".to_string()
                        } else {
                            binding.protocols.join(",")
                        },
                    ),
                    ("worker_count", binding.worker_count.to_string()),
                    ("reuse_port_enabled", opt_or_dash(binding.reuse_port_enabled)),
                    ("advertise_alt_svc", opt_or_dash(binding.advertise_alt_svc)),
                    ("alt_svc_max_age_secs", opt_or_dash(binding.alt_svc_max_age_secs)),
                    (
                        "http3_max_concurrent_streams",
                        opt_or_dash(binding.http3_max_concurrent_streams),
                    ),
                    ("http3_stream_buffer_size", opt_or_dash(binding.http3_stream_buffer_size)),
                    (
                        "http3_active_connection_id_limit",
                        opt_or_dash(binding.http3_active_connection_id_limit),
                    ),
                    ("http3_retry", opt_or_dash(binding.http3_retry)),
                    ("http3_host_key_path", opt_path_or_dash(binding.http3_host_key_path.as_ref())),
                    ("http3_gso", opt_or_dash(binding.http3_gso)),
                    ("http3_early_data_enabled", opt_or_dash(binding.http3_early_data_enabled)),
                ],
            );
        }

        if let Some(http3) = &listener.http3_runtime {
            print_record(
                "status_listener_http3",
                [
                    ("listener", listener.listener_name.clone()),
                    ("listener_id", listener.listener_id.clone()),
                    ("active_connections", http3.active_connections.to_string()),
                    ("active_request_streams", http3.active_request_streams.to_string()),
                    ("retry_issued_total", http3.retry_issued_total.to_string()),
                    ("retry_failed_total", http3.retry_failed_total.to_string()),
                    ("request_accept_errors_total", http3.request_accept_errors_total.to_string()),
                    (
                        "request_resolve_errors_total",
                        http3.request_resolve_errors_total.to_string(),
                    ),
                    (
                        "request_body_stream_errors_total",
                        http3.request_body_stream_errors_total.to_string(),
                    ),
                    (
                        "response_stream_errors_total",
                        http3.response_stream_errors_total.to_string(),
                    ),
                    (
                        "connection_close_version_mismatch_total",
                        http3.connection_close_version_mismatch_total.to_string(),
                    ),
                    (
                        "connection_close_transport_error_total",
                        http3.connection_close_transport_error_total.to_string(),
                    ),
                    (
                        "connection_close_connection_closed_total",
                        http3.connection_close_connection_closed_total.to_string(),
                    ),
                    (
                        "connection_close_application_closed_total",
                        http3.connection_close_application_closed_total.to_string(),
                    ),
                    (
                        "connection_close_reset_total",
                        http3.connection_close_reset_total.to_string(),
                    ),
                    (
                        "connection_close_timed_out_total",
                        http3.connection_close_timed_out_total.to_string(),
                    ),
                    (
                        "connection_close_locally_closed_total",
                        http3.connection_close_locally_closed_total.to_string(),
                    ),
                    (
                        "connection_close_cids_exhausted_total",
                        http3.connection_close_cids_exhausted_total.to_string(),
                    ),
                ],
            );
        }
    }
}

fn opt_or_dash<T: ToString>(value: Option<T>) -> String {
    value.map(|value| value.to_string()).unwrap_or_else(|| "-".to_string())
}

fn opt_path_or_dash(value: Option<&std::path::PathBuf>) -> String {
    value.map(|path| path.display().to_string()).unwrap_or_else(|| "-".to_string())
}
