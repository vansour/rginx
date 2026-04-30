use std::collections::{BTreeSet, HashMap};
use std::net::SocketAddr;
use std::time::Duration;

use instant_acme::RetryPolicy;
use rginx_core::{AcmeSettings, ConfigSnapshot, ManagedCertificateSpec};
use rginx_http::{TlsCertificateStatusSnapshot, tls_runtime_snapshot_for_config};

const DAY_SECS: u64 = 86_400;
const ACME_ORDER_POLL_TIMEOUT: Duration = Duration::from_secs(180);
const ACME_ORDER_POLL_INITIAL_DELAY: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateFailure {
    pub scope: String,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueSummary {
    pub total: usize,
    pub issued: usize,
    pub skipped: usize,
    pub failures: Vec<CertificateFailure>,
}

impl IssueSummary {
    pub(crate) fn new(total: usize) -> Self {
        Self { total, issued: 0, skipped: 0, failures: Vec::new() }
    }

    pub fn is_success(&self) -> bool {
        self.failures.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReconcileReason {
    MissingCertificate,
    MissingPrivateKey,
    MissingCertificateMetadata,
    SanMismatch { expected: Vec<String>, actual: Vec<String> },
    Expiring { remaining_days: i64, renew_before_days: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReconcilePlan {
    pub(crate) reason: ReconcileReason,
}

impl ReconcilePlan {
    pub(crate) fn describe(&self) -> String {
        match &self.reason {
            ReconcileReason::MissingCertificate => "certificate file is missing".to_string(),
            ReconcileReason::MissingPrivateKey => "private key file is missing".to_string(),
            ReconcileReason::MissingCertificateMetadata => {
                "certificate metadata could not be inspected".to_string()
            }
            ReconcileReason::SanMismatch { expected, actual } => {
                format!("certificate SAN mismatch expected={expected:?} actual={actual:?}")
            }
            ReconcileReason::Expiring { remaining_days, renew_before_days } => {
                format!(
                    "certificate expires in {remaining_days}d (renew_before={renew_before_days}d)"
                )
            }
        }
    }
}

pub(crate) fn certificate_status_index(
    config: &ConfigSnapshot,
) -> HashMap<String, TlsCertificateStatusSnapshot> {
    let mut index = HashMap::new();
    for status in tls_runtime_snapshot_for_config(config).certificates {
        if let Some(scope) = status.scope.strip_prefix("vhost:") {
            index.insert(scope.to_string(), status.clone());
        }
        index.insert(status.scope.clone(), status);
    }
    index
}

pub(crate) fn http01_listener_addrs(config: &ConfigSnapshot) -> Vec<SocketAddr> {
    config
        .listeners
        .iter()
        .filter(|listener| !listener.tls_enabled() && listener.server.listen_addr.port() == 80)
        .map(|listener| listener.server.listen_addr)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(crate) fn plan_reconcile(
    spec: &ManagedCertificateSpec,
    status: Option<&TlsCertificateStatusSnapshot>,
    settings: &AcmeSettings,
) -> Option<ReconcilePlan> {
    let status = match status {
        Some(status) => status,
        None => return Some(ReconcilePlan { reason: ReconcileReason::MissingCertificate }),
    };

    if !spec.key_path.is_file() {
        return Some(ReconcilePlan { reason: ReconcileReason::MissingPrivateKey });
    }

    if status.fingerprint_sha256.is_none() {
        return Some(ReconcilePlan {
            reason: if status.cert_path.is_file() {
                ReconcileReason::MissingCertificateMetadata
            } else {
                ReconcileReason::MissingCertificate
            },
        });
    }

    let expected = normalized_domains(&spec.domains);
    let actual = normalized_domains(&status.san_dns_names);
    if expected != actual {
        return Some(ReconcilePlan { reason: ReconcileReason::SanMismatch { expected, actual } });
    }

    let renew_before_days = renew_before_days(settings);
    match status.expires_in_days {
        Some(remaining_days) if remaining_days > renew_before_days => None,
        Some(remaining_days) => Some(ReconcilePlan {
            reason: ReconcileReason::Expiring { remaining_days, renew_before_days },
        }),
        None => Some(ReconcilePlan { reason: ReconcileReason::MissingCertificateMetadata }),
    }
}

pub(crate) fn acme_poll_retry_policy() -> RetryPolicy {
    RetryPolicy::new().initial_delay(ACME_ORDER_POLL_INITIAL_DELAY).timeout(ACME_ORDER_POLL_TIMEOUT)
}

fn renew_before_days(settings: &AcmeSettings) -> i64 {
    let secs = settings.renew_before.as_secs();
    secs.div_ceil(DAY_SECS) as i64
}

fn normalized_domains(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
