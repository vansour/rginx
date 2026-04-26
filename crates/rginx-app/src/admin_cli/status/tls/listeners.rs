use crate::admin_cli::render::{print_record, render_optional_string_list};

use super::{render_optional_path, render_optional_value, render_string_list};

pub(super) fn print_status_tls_listeners(listeners: &[rginx_http::TlsListenerStatusSnapshot]) {
    for listener in listeners {
        print_record(
            "status_tls_listener",
            [
                ("listener", listener.listener_name.clone()),
                ("listener_id", listener.listener_id.clone()),
                ("listen", listener.listen_addr.to_string()),
                ("tls", listener.tls_enabled.to_string()),
                ("http3_enabled", listener.http3_enabled.to_string()),
                ("http3_listen", render_optional_value(listener.http3_listen_addr)),
                (
                    "default_certificate",
                    listener.default_certificate.clone().unwrap_or_else(|| "-".to_string()),
                ),
                (
                    "tcp_versions",
                    listener
                        .versions
                        .as_ref()
                        .map(|versions| render_optional_string_list(Some(versions)))
                        .unwrap_or_else(|| "-".to_string()),
                ),
                ("tcp_alpn_protocols", render_string_list(&listener.alpn_protocols)),
                ("http3_versions", render_string_list(&listener.http3_versions)),
                ("http3_alpn_protocols", render_string_list(&listener.http3_alpn_protocols)),
                (
                    "http3_max_concurrent_streams",
                    render_optional_value(listener.http3_max_concurrent_streams),
                ),
                (
                    "http3_stream_buffer_size",
                    render_optional_value(listener.http3_stream_buffer_size),
                ),
                (
                    "http3_active_connection_id_limit",
                    render_optional_value(listener.http3_active_connection_id_limit),
                ),
                ("http3_retry", render_optional_value(listener.http3_retry)),
                (
                    "http3_host_key_path",
                    render_optional_path(listener.http3_host_key_path.as_deref()),
                ),
                ("http3_gso", render_optional_value(listener.http3_gso)),
                (
                    "http3_early_data_enabled",
                    render_optional_value(listener.http3_early_data_enabled),
                ),
                ("sni_names", render_string_list(&listener.sni_names)),
                (
                    "client_auth_mode",
                    listener.client_auth_mode.clone().unwrap_or_else(|| "-".to_string()),
                ),
                (
                    "client_auth_verify_depth",
                    render_optional_value(listener.client_auth_verify_depth),
                ),
                ("client_auth_crl_configured", listener.client_auth_crl_configured.to_string()),
            ],
        );
    }
}
