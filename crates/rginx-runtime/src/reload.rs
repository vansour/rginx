use std::sync::Arc;

use rginx_core::{ConfigSnapshot, Result};

use crate::state::RuntimeState;

pub struct PendingReload {
    pub current_config: Arc<ConfigSnapshot>,
    pub current_revision: u64,
    pub next_config: ConfigSnapshot,
}

pub struct ReloadSuccess {
    pub config: Arc<ConfigSnapshot>,
    pub revision: u64,
}

pub async fn prepare_reload(state: &RuntimeState) -> Result<PendingReload> {
    let current_config = state.current_config().await;
    let current_revision = state.http.current_revision().await;
    let next_config = match rginx_config::load_and_compile(&state.config_path) {
        Ok(config) => config,
        Err(error) => {
            state.http.record_reload_failure(error.to_string(), current_revision);
            return Err(error);
        }
    };

    if let Err(error) =
        rginx_http::validate_config_transition(current_config.as_ref(), &next_config)
    {
        state.http.record_reload_failure(error.to_string(), current_revision);
        return Err(error);
    }

    Ok(PendingReload { current_config, current_revision, next_config })
}

pub async fn commit_reload(state: &RuntimeState, pending: PendingReload) -> Result<ReloadSuccess> {
    let current_config = pending.current_config;
    let current_revision = pending.current_revision;
    let config = match state.http.replace(pending.next_config).await {
        Ok(config) => config,
        Err(error) => {
            state.http.record_reload_failure(error.to_string(), current_revision);
            return Err(error);
        }
    };
    let revision = state.http.current_revision().await;
    let tls_certificate_changes =
        describe_tls_certificate_changes(current_config.as_ref(), config.as_ref());
    state.http.record_reload_success(revision, tls_certificate_changes);
    Ok(ReloadSuccess { config, revision })
}

pub async fn reload(state: &RuntimeState) -> Result<ReloadSuccess> {
    let pending = prepare_reload(state).await?;
    commit_reload(state, pending).await
}

fn describe_tls_certificate_changes(
    previous: &ConfigSnapshot,
    next: &ConfigSnapshot,
) -> Vec<String> {
    let previous = rginx_http::tls_runtime_snapshot_for_config(previous)
        .certificates
        .into_iter()
        .map(|certificate| {
            (certificate.scope, certificate.fingerprint_sha256.unwrap_or_else(|| "-".to_string()))
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let next = rginx_http::tls_runtime_snapshot_for_config(next)
        .certificates
        .into_iter()
        .map(|certificate| {
            (certificate.scope, certificate.fingerprint_sha256.unwrap_or_else(|| "-".to_string()))
        })
        .collect::<std::collections::BTreeMap<_, _>>();

    previous
        .keys()
        .chain(next.keys())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter_map(|scope| match (previous.get(scope), next.get(scope)) {
            (Some(previous_fingerprint), Some(next_fingerprint))
                if previous_fingerprint == next_fingerprint =>
            {
                None
            }
            (Some(previous_fingerprint), Some(next_fingerprint)) => {
                Some(format!("{scope}:{previous_fingerprint}->{next_fingerprint}"))
            }
            (None, Some(next_fingerprint)) => Some(format!("{scope}:->{next_fingerprint}")),
            (Some(previous_fingerprint), None) => {
                Some(format!("{scope}:{previous_fingerprint}->-"))
            }
            (None, None) => None,
        })
        .collect::<Vec<_>>()
}
