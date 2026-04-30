use std::collections::HashMap;
use std::path::PathBuf;

use super::tls::TlsCheckDetails;

pub(super) struct AcmeCheckDetails {
    pub(super) enabled: bool,
    pub(super) directory_url: Option<String>,
    pub(super) state_dir: Option<PathBuf>,
    pub(super) renew_before_days: Option<u64>,
    pub(super) poll_interval_secs: Option<u64>,
    pub(super) managed_certificates: Vec<AcmeManagedCertificateCheck>,
}

pub(super) struct AcmeManagedCertificateCheck {
    pub(super) scope: String,
    pub(super) domains: Vec<String>,
    pub(super) managed: bool,
    pub(super) challenge_type: String,
    pub(super) cert_path: PathBuf,
    pub(super) key_path: PathBuf,
    pub(super) next_renewal_unix_ms: Option<u64>,
}

pub(super) fn acme_check_details(
    config: &rginx_config::ConfigSnapshot,
    tls: &TlsCheckDetails,
) -> AcmeCheckDetails {
    let certificate_statuses = tls
        .certificates
        .iter()
        .flat_map(|status| {
            let mut entries = vec![(status.scope.as_str(), status)];
            if let Some(scope) = status.scope.strip_prefix("vhost:") {
                entries.push((scope, status));
            }
            entries
        })
        .collect::<HashMap<_, _>>();

    AcmeCheckDetails {
        enabled: config.acme.is_some(),
        directory_url: config.acme.as_ref().map(|settings| settings.directory_url.clone()),
        state_dir: config.acme.as_ref().map(|settings| settings.state_dir.clone()),
        renew_before_days: config
            .acme
            .as_ref()
            .map(|settings| settings.renew_before.as_secs().div_ceil(86_400)),
        poll_interval_secs: config.acme.as_ref().map(|settings| settings.poll_interval.as_secs()),
        managed_certificates: config
            .managed_certificates
            .iter()
            .map(|spec| AcmeManagedCertificateCheck {
                scope: spec.scope.clone(),
                domains: spec.domains.clone(),
                managed: true,
                challenge_type: spec.challenge.as_str().to_string(),
                cert_path: spec.cert_path.clone(),
                key_path: spec.key_path.clone(),
                next_renewal_unix_ms: certificate_statuses
                    .get(spec.scope.as_str())
                    .and_then(|status| status.not_after_unix_ms)
                    .and_then(|not_after_unix_ms| {
                        config.acme.as_ref().map(|settings| {
                            not_after_unix_ms.saturating_sub(
                                u64::try_from(settings.renew_before.as_millis())
                                    .unwrap_or(u64::MAX),
                            )
                        })
                    }),
            })
            .collect(),
    }
}
