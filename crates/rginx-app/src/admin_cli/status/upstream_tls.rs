use crate::admin_cli::render::{print_record, render_optional_string_list};

pub(super) fn print_status_upstream_tls(upstream_tls: &[rginx_http::UpstreamTlsStatusSnapshot]) {
    for upstream_tls in upstream_tls {
        print_record(
            "status_upstream_tls",
            [
                ("upstream", upstream_tls.upstream_name.clone()),
                ("protocol", upstream_tls.protocol.clone()),
                ("verify_mode", upstream_tls.verify_mode.clone()),
                ("tls_versions", render_optional_string_list(upstream_tls.tls_versions.as_deref())),
                ("server_name_enabled", upstream_tls.server_name_enabled.to_string()),
                (
                    "server_name_override",
                    upstream_tls.server_name_override.clone().unwrap_or_else(|| "-".to_string()),
                ),
                (
                    "verify_depth",
                    upstream_tls
                        .verify_depth
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                ),
                ("crl_configured", upstream_tls.crl_configured.to_string()),
                ("client_identity_configured", upstream_tls.client_identity_configured.to_string()),
            ],
        );
    }
}
