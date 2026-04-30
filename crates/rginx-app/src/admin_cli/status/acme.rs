use crate::admin_cli::render::print_record;

pub(super) fn print_status_acme(acme: &rginx_http::AcmeRuntimeSnapshot) {
    let active_retry_count = acme
        .managed_certificates
        .iter()
        .filter(|certificate| certificate.retry_after_unix_ms.is_some())
        .count();
    let last_error_count = acme
        .managed_certificates
        .iter()
        .filter(|certificate| certificate.last_error.is_some())
        .count();

    print_record(
        "status_acme",
        [
            ("enabled", acme.enabled.to_string()),
            ("directory_url", acme.directory_url.clone().unwrap_or_else(|| "-".to_string())),
            ("managed_certificates", acme.managed_certificates.len().to_string()),
            ("retry_pending", active_retry_count.to_string()),
            ("last_errors", last_error_count.to_string()),
        ],
    );

    for certificate in &acme.managed_certificates {
        print_record(
            "status_acme_certificate",
            [
                ("scope", certificate.scope.clone()),
                (
                    "domains",
                    if certificate.domains.is_empty() {
                        "-".to_string()
                    } else {
                        certificate.domains.join(",")
                    },
                ),
                ("managed", certificate.managed.to_string()),
                ("challenge_type", certificate.challenge_type.clone()),
                ("cert_path", certificate.cert_path.display().to_string()),
                ("key_path", certificate.key_path.display().to_string()),
                (
                    "last_success_unix_ms",
                    certificate
                        .last_success_unix_ms
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                ),
                (
                    "next_renewal_unix_ms",
                    certificate
                        .next_renewal_unix_ms
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                ),
                ("refreshes_total", certificate.refreshes_total.to_string()),
                ("failures_total", certificate.failures_total.to_string()),
                (
                    "retry_after_unix_ms",
                    certificate
                        .retry_after_unix_ms
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                ),
                ("last_error", certificate.last_error.clone().unwrap_or_else(|| "-".to_string())),
            ],
        );
    }
}
