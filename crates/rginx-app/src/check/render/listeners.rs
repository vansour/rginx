use super::super::summary::CheckSummary;
use super::{render_enabled, render_string_list};

pub(super) fn print_listener_details(summary: &CheckSummary) {
    for listener in &summary.listeners {
        println!(
            "check_listener id={} name={} listen={} transport_bindings={} tls={} http3={} proxy_protocol={} default_certificate={} keep_alive={} max_connections={} access_log_format_configured={}",
            listener.id,
            listener.name,
            listener.listen_addr,
            listener.binding_count,
            render_enabled(listener.tls_enabled),
            render_enabled(listener.http3_enabled),
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
                render_string_list(&binding.protocols),
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
}
