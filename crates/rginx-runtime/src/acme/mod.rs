use rginx_core::{ConfigSnapshot, Error, Result};
use rginx_http::SharedState;
use tokio::sync::watch;

mod account;
mod challenge;
mod lock;
mod order;
mod scheduler;
mod storage;
#[cfg(test)]
mod tests;
mod types;

pub use types::{CertificateFailure, IssueSummary};

use account::load_or_create_account;
use challenge::{ChallengeBackend, TemporaryChallengeServer};
use lock::AcmeStateLock;
use order::issue_and_store_managed_certificate;
use types::{certificate_status_index, plan_reconcile};

pub async fn run(state: SharedState, shutdown: watch::Receiver<bool>) {
    scheduler::run(state, shutdown).await;
}

pub async fn issue_once(config: &ConfigSnapshot) -> Result<IssueSummary> {
    let Some(settings) = config.acme.as_ref() else {
        return Err(Error::Config(
            "`rginx acme issue --once` requires top-level acme configuration".to_string(),
        ));
    };

    let certificate_statuses = certificate_status_index(config);
    let mut summary = IssueSummary::new(config.managed_certificates.len());
    let mut pending = Vec::new();

    for spec in &config.managed_certificates {
        match plan_reconcile(spec, certificate_statuses.get(&spec.scope), settings) {
            Some(plan) => pending.push((spec, plan)),
            None => {
                summary.skipped += 1;
                tracing::info!(
                    scope = %spec.scope,
                    "managed certificate already satisfies current ACME spec"
                );
            }
        }
    }

    if pending.is_empty() {
        return Ok(summary);
    }

    let _lock = AcmeStateLock::acquire(settings)?;
    let challenge_server = TemporaryChallengeServer::bind_for_config(config).await?;
    let issue_result = async {
        let account = load_or_create_account(settings).await?;
        let challenge_backend: std::sync::Arc<dyn ChallengeBackend> = challenge_server.backend();

        for (spec, plan) in pending {
            tracing::info!(
                scope = %spec.scope,
                reason = %plan.describe(),
                "issuing managed ACME certificate via one-shot flow"
            );
            match issue_and_store_managed_certificate(spec, &account, challenge_backend.clone())
                .await
            {
                Ok(()) => {
                    summary.issued += 1;
                    tracing::info!(scope = %spec.scope, "managed ACME certificate issued");
                }
                Err(error) => {
                    summary.failures.push(types::CertificateFailure {
                        scope: spec.scope.clone(),
                        error: error.to_string(),
                    });
                    tracing::warn!(scope = %spec.scope, %error, "managed ACME issuance failed");
                }
            }
        }

        Ok(summary)
    }
    .await;

    challenge_server.shutdown().await;
    issue_result
}
