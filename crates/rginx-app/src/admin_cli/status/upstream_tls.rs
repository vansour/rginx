use crate::admin_cli::render::{print_record, render_optional_string_list};

pub(super) fn print_status_upstream_tls(snapshots: &[rginx_http::UpstreamTlsStatusSnapshot]) {
    for snapshot in snapshots {
        print_record(
            "status_upstream_tls",
            [
                ("upstream", snapshot.upstream_name.clone()),
                ("protocol", snapshot.protocol.clone()),
                ("verify_mode", snapshot.verify_mode.clone()),
                ("tls_versions", render_optional_string_list(snapshot.tls_versions.as_deref())),
                ("server_name_enabled", snapshot.server_name_enabled.to_string()),
                (
                    "server_name_override",
                    snapshot.server_name_override.clone().unwrap_or_else(|| "-".to_string()),
                ),
                (
                    "verify_depth",
                    snapshot
                        .verify_depth
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                ),
                ("crl_configured", snapshot.crl_configured.to_string()),
                ("client_identity_configured", snapshot.client_identity_configured.to_string()),
            ],
        );
    }
}
