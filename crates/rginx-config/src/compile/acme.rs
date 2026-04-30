use std::path::Path;
use std::time::Duration;

use rginx_core::{AcmeChallengeType, AcmeSettings, ManagedCertificateSpec, VirtualHostTls};

use crate::model::{AcmeChallengeConfig, AcmeConfig, VirtualHostAcmeConfig};

const DEFAULT_ACME_RENEW_BEFORE_DAYS: u64 = 30;
const DEFAULT_ACME_POLL_INTERVAL_SECS: u64 = 3600;

pub(super) fn compile_global_acme(
    acme: Option<AcmeConfig>,
    base_dir: &Path,
) -> Option<AcmeSettings> {
    let acme = acme?;
    let directory_url = acme.directory_url.trim().to_string();
    let contacts = acme.contacts.into_iter().map(|contact| contact.trim().to_string()).collect();
    let state_dir = super::resolve_path(base_dir, acme.state_dir);
    let renew_before = Duration::from_secs(
        acme.renew_before_days.unwrap_or(DEFAULT_ACME_RENEW_BEFORE_DAYS) * 86_400,
    );
    let poll_interval =
        Duration::from_secs(acme.poll_interval_secs.unwrap_or(DEFAULT_ACME_POLL_INTERVAL_SECS));

    Some(AcmeSettings { directory_url, contacts, state_dir, renew_before, poll_interval })
}

pub(super) fn compile_managed_certificate_spec(
    scope: String,
    tls: &VirtualHostTls,
    acme: Option<VirtualHostAcmeConfig>,
) -> Option<ManagedCertificateSpec> {
    let acme = acme?;
    let domains = acme
        .domains
        .into_iter()
        .map(|domain| domain.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();
    let challenge = match acme.challenge.unwrap_or(AcmeChallengeConfig::Http01) {
        AcmeChallengeConfig::Http01 => AcmeChallengeType::Http01,
    };

    Some(ManagedCertificateSpec {
        scope,
        domains,
        cert_path: tls.cert_path.clone(),
        key_path: tls.key_path.clone(),
        challenge,
    })
}
