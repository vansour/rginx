use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rginx_http::SharedState;
use tokio::sync::watch;

use super::account::load_or_create_account;
use super::challenge::{ChallengeBackend, RuntimeChallengeBackend};
use super::order::issue_and_store_managed_certificate;
use super::types::{certificate_status_index, http01_listener_addrs, plan_reconcile};

const DEFAULT_IDLE_INTERVAL: Duration = Duration::from_secs(300);
const INITIAL_RETRY_DELAY: Duration = Duration::from_secs(60);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Clone)]
struct RetryState {
    failures: u32,
    next_retry_at: Instant,
}

#[derive(Default)]
struct RetryBackoff {
    entries: HashMap<String, RetryState>,
}

impl RetryBackoff {
    fn retain_scopes(&mut self, scopes: &HashSet<String>) {
        self.entries.retain(|scope, _| scopes.contains(scope));
    }

    fn remaining_delay(&self, scope: &str) -> Option<Duration> {
        self.entries
            .get(scope)
            .and_then(|state| state.next_retry_at.checked_duration_since(Instant::now()))
    }

    fn record_success(&mut self, scope: &str) {
        self.entries.remove(scope);
    }

    fn record_failure(&mut self, scope: &str) -> Duration {
        let failures =
            self.entries.get(scope).map(|state| state.failures.saturating_add(1)).unwrap_or(1);
        let exponent = failures.saturating_sub(1).min(8);
        let retry_delay = std::cmp::min(INITIAL_RETRY_DELAY * (1 << exponent), MAX_RETRY_DELAY);
        self.entries.insert(
            scope.to_string(),
            RetryState { failures, next_retry_at: Instant::now() + retry_delay },
        );
        retry_delay
    }
}

pub(super) async fn run(state: SharedState, mut shutdown: watch::Receiver<bool>) {
    let mut revisions = state.subscribe_updates();
    let mut interval = tokio::time::interval(current_poll_interval(&state).await);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut retry_backoff = RetryBackoff::default();

    if let Err(error) = reconcile_managed_certificates(&state, &mut retry_backoff).await {
        tracing::warn!(%error, "initial ACME reconcile failed");
    }

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                match changed {
                    Ok(()) if *shutdown.borrow() => break,
                    Ok(()) => continue,
                    Err(_) => break,
                }
            }
            changed = revisions.changed() => {
                if changed.is_err() {
                    break;
                }
                interval = tokio::time::interval(current_poll_interval(&state).await);
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                if let Err(error) = reconcile_managed_certificates(&state, &mut retry_backoff).await {
                    tracing::warn!(%error, "ACME reconcile after reload failed");
                }
            }
            _ = interval.tick() => {
                if let Err(error) = reconcile_managed_certificates(&state, &mut retry_backoff).await {
                    tracing::warn!(%error, "periodic ACME reconcile failed");
                }
            }
        }
    }

    tracing::info!("managed ACME reconciler stopped");
}

async fn current_poll_interval(state: &SharedState) -> Duration {
    state
        .current_config()
        .await
        .acme
        .as_ref()
        .map(|settings| settings.poll_interval)
        .unwrap_or(DEFAULT_IDLE_INTERVAL)
}

async fn reconcile_managed_certificates(
    state: &SharedState,
    retry_backoff: &mut RetryBackoff,
) -> Result<(), String> {
    let config = state.current_config().await;
    let Some(settings) = config.acme.as_ref() else {
        retry_backoff.entries.clear();
        return Ok(());
    };
    if config.managed_certificates.is_empty() {
        retry_backoff.entries.clear();
        return Ok(());
    }

    let active_scopes =
        config.managed_certificates.iter().map(|spec| spec.scope.clone()).collect::<HashSet<_>>();
    retry_backoff.retain_scopes(&active_scopes);

    let certificate_statuses = certificate_status_index(config.as_ref());
    let pending = config
        .managed_certificates
        .iter()
        .filter_map(|spec| {
            plan_reconcile(spec, certificate_statuses.get(&spec.scope), settings)
                .map(|plan| (spec, plan))
        })
        .collect::<Vec<_>>();
    if pending.is_empty() {
        return Ok(());
    }

    if http01_listener_addrs(config.as_ref()).is_empty() {
        for (spec, plan) in pending {
            tracing::warn!(
                scope = %spec.scope,
                reason = %plan.describe(),
                "managed ACME reconcile deferred because no plain HTTP listener is configured on port 80"
            );
        }
        return Ok(());
    }

    let account = load_or_create_account(settings).await.map_err(|error| error.to_string())?;
    let challenge_backend: Arc<dyn ChallengeBackend> =
        Arc::new(RuntimeChallengeBackend::new(state.clone()));
    let mut tls_acceptors_changed = false;

    for (spec, plan) in pending {
        if let Some(delay) = retry_backoff.remaining_delay(&spec.scope) {
            tracing::debug!(
                scope = %spec.scope,
                retry_after_secs = delay.as_secs(),
                "managed ACME reconcile is waiting for retry backoff"
            );
            continue;
        }

        tracing::info!(
            scope = %spec.scope,
            reason = %plan.describe(),
            "reconciling managed ACME certificate"
        );
        match issue_and_store_managed_certificate(spec, &account, challenge_backend.clone()).await {
            Ok(()) => {
                retry_backoff.record_success(&spec.scope);
                tls_acceptors_changed = true;
                tracing::info!(scope = %spec.scope, "managed ACME certificate refreshed");
            }
            Err(error) => {
                let retry_delay = retry_backoff.record_failure(&spec.scope);
                tracing::warn!(
                    scope = %spec.scope,
                    %error,
                    retry_after_secs = retry_delay.as_secs(),
                    "managed ACME reconcile failed"
                );
            }
        }
    }

    if tls_acceptors_changed {
        state.refresh_tls_acceptors_from_current_config().await.map_err(|error| {
            format!("failed to rebuild TLS acceptors after ACME certificate refresh: {error}")
        })?;
    }

    Ok(())
}
