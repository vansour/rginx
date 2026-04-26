use crate::admin_cli::render::print_record;

use super::{render_optional_path, render_optional_value, render_string_list};

pub(super) fn print_status_tls_ocsp(ocsp_entries: &[rginx_http::TlsOcspStatusSnapshot]) {
    for ocsp in ocsp_entries {
        print_record(
            "status_tls_ocsp",
            [
                ("scope", ocsp.scope.clone()),
                ("cert_path", ocsp.cert_path.display().to_string()),
                ("staple_path", render_optional_path(ocsp.ocsp_staple_path.as_deref())),
                ("responder_urls", render_string_list(&ocsp.responder_urls)),
                ("nonce_mode", ocsp.nonce_mode.clone()),
                ("responder_policy", ocsp.responder_policy.clone()),
                ("cache_loaded", ocsp.cache_loaded.to_string()),
                ("cache_size_bytes", render_optional_value(ocsp.cache_size_bytes)),
                ("cache_modified_unix_ms", render_optional_value(ocsp.cache_modified_unix_ms)),
                ("auto_refresh_enabled", ocsp.auto_refresh_enabled.to_string()),
                ("last_refresh_unix_ms", render_optional_value(ocsp.last_refresh_unix_ms)),
                ("refreshes_total", ocsp.refreshes_total.to_string()),
                ("failures_total", ocsp.failures_total.to_string()),
                ("last_error", ocsp.last_error.clone().unwrap_or_else(|| "-".to_string())),
            ],
        );
    }
}
