use std::sync::Arc;

use rginx_core::{ConfigSnapshot, Result};

use crate::state::RuntimeState;

pub struct ReloadSuccess {
    pub config: Arc<ConfigSnapshot>,
    pub revision: u64,
}

pub async fn reload(state: &RuntimeState) -> Result<ReloadSuccess> {
    let current_config = state.current_config().await;
    let current_revision = state.http.current_revision().await;
    let config = match rginx_config::load_and_compile(&state.config_path) {
        Ok(config) => config,
        Err(error) => {
            state.http.record_reload_failure(error.to_string(), current_revision);
            return Err(error);
        }
    };
    let config = match state.http.replace(config).await {
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

fn describe_tls_certificate_changes(
    previous: &ConfigSnapshot,
    next: &ConfigSnapshot,
) -> Vec<String> {
    let previous = rginx_http::tls_runtime_snapshot_for_config(previous)
        .certificates
        .into_iter()
        .map(|certificate| (certificate.scope, certificate.fingerprint_sha256))
        .collect::<std::collections::HashMap<_, _>>();
    let mut changes = rginx_http::tls_runtime_snapshot_for_config(next)
        .certificates
        .into_iter()
        .filter_map(|certificate| {
            let next_fingerprint = certificate.fingerprint_sha256?;
            match previous.get(&certificate.scope) {
                Some(Some(previous_fingerprint)) if previous_fingerprint == &next_fingerprint => {
                    None
                }
                Some(Some(previous_fingerprint)) => Some(format!(
                    "{}:{}->{}",
                    certificate.scope, previous_fingerprint, next_fingerprint
                )),
                _ => Some(format!("{}:- ->{}", certificate.scope, next_fingerprint)),
            }
        })
        .collect::<Vec<_>>();
    changes.sort();
    changes
}
