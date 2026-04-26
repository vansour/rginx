use crate::admin_cli::render::print_record;

use super::{render_optional_value, render_string_list};

pub(super) fn print_status_tls_certificates(
    certificates: &[rginx_http::TlsCertificateStatusSnapshot],
) {
    for certificate in certificates {
        print_record(
            "status_tls_certificate",
            [
                ("scope", certificate.scope.clone()),
                ("cert_path", certificate.cert_path.display().to_string()),
                ("server_names", render_string_list(&certificate.server_names)),
                (
                    "sha256",
                    certificate.fingerprint_sha256.clone().unwrap_or_else(|| "-".to_string()),
                ),
                ("subject", certificate.subject.clone().unwrap_or_else(|| "-".to_string())),
                ("issuer", certificate.issuer.clone().unwrap_or_else(|| "-".to_string())),
                ("serial", certificate.serial_number.clone().unwrap_or_else(|| "-".to_string())),
                ("san_dns_names", render_string_list(&certificate.san_dns_names)),
                (
                    "ski",
                    certificate.subject_key_identifier.clone().unwrap_or_else(|| "-".to_string()),
                ),
                (
                    "aki",
                    certificate.authority_key_identifier.clone().unwrap_or_else(|| "-".to_string()),
                ),
                ("is_ca", render_optional_value(certificate.is_ca)),
                ("path_len_constraint", render_optional_value(certificate.path_len_constraint)),
                ("key_usage", certificate.key_usage.clone().unwrap_or_else(|| "-".to_string())),
                ("extended_key_usage", render_string_list(&certificate.extended_key_usage)),
                ("not_before_unix_ms", render_optional_value(certificate.not_before_unix_ms)),
                ("not_after_unix_ms", render_optional_value(certificate.not_after_unix_ms)),
                ("expires_in_days", render_optional_value(certificate.expires_in_days)),
                ("chain_length", certificate.chain_length.to_string()),
                ("chain_subjects", render_string_list(&certificate.chain_subjects)),
                (
                    "chain_diagnostics",
                    if certificate.chain_diagnostics.is_empty() {
                        "-".to_string()
                    } else {
                        certificate.chain_diagnostics.join("|")
                    },
                ),
                (
                    "selected_as_default_for_listeners",
                    render_string_list(&certificate.selected_as_default_for_listeners),
                ),
                ("ocsp_staple_configured", certificate.ocsp_staple_configured.to_string()),
                (
                    "additional_certificate_count",
                    certificate.additional_certificate_count.to_string(),
                ),
            ],
        );
    }
}
