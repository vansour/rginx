use super::super::summary::CheckSummary;
use super::render_string_list;

pub(super) fn print_acme_details(summary: &CheckSummary) {
    println!(
        "acme_details=enabled={} directory_url={} state_dir={} renew_before_days={} poll_interval_secs={} managed_certificates={}",
        summary.acme.enabled,
        summary.acme.directory_url.as_deref().unwrap_or("-"),
        summary
            .acme
            .state_dir
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".to_string()),
        summary
            .acme
            .renew_before_days
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        summary
            .acme
            .poll_interval_secs
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        summary.acme.managed_certificates.len(),
    );

    for certificate in &summary.acme.managed_certificates {
        println!(
            "acme_certificate scope={} managed={} domains={} challenge_type={} cert_path={} key_path={} next_renewal_unix_ms={}",
            certificate.scope,
            certificate.managed,
            render_string_list(&certificate.domains),
            certificate.challenge_type,
            certificate.cert_path.display(),
            certificate.key_path.display(),
            certificate
                .next_renewal_unix_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
        );
    }
}
