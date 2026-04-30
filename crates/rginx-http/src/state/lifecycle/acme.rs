use super::super::*;
use std::time::{Duration, SystemTime};

impl SharedState {
    pub fn register_acme_http01_challenge(
        &self,
        token: impl Into<String>,
        key_authorization: impl Into<String>,
    ) {
        self.acme_http01_challenges
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(token.into(), key_authorization.into());
    }

    pub fn unregister_acme_http01_challenge(&self, token: &str) {
        self.acme_http01_challenges
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(token);
    }

    pub fn acme_http01_response(&self, token: &str) -> Option<String> {
        self.acme_http01_challenges
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(token)
            .cloned()
    }

    pub fn record_acme_refresh_success(&self, scope: &str) {
        let mut statuses =
            self.acme_statuses.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = statuses.entry(scope.to_string()).or_default();
        entry.last_success_unix_ms = Some(unix_time_ms(SystemTime::now()));
        entry.refreshes_total += 1;
        entry.retry_after_unix_ms = None;
        entry.last_error = None;
        self.mark_snapshot_changed_components(true, false, false, false, false);
        self.notify_snapshot_waiters();
    }

    pub fn record_acme_refresh_failure(
        &self,
        scope: &str,
        error: impl Into<String>,
        retry_after: Option<Duration>,
    ) {
        let mut statuses =
            self.acme_statuses.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = statuses.entry(scope.to_string()).or_default();
        entry.failures_total += 1;
        entry.retry_after_unix_ms =
            retry_after.and_then(|delay| SystemTime::now().checked_add(delay).map(unix_time_ms));
        entry.last_error = Some(error.into());
        self.mark_snapshot_changed_components(true, false, false, false, false);
        self.notify_snapshot_waiters();
    }

    pub(super) fn sync_acme_statuses(&self, config: &ConfigSnapshot) {
        let managed_scopes = config
            .managed_certificates
            .iter()
            .map(|spec| spec.scope.as_str())
            .collect::<std::collections::HashSet<_>>();
        self.acme_statuses
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|scope, _| managed_scopes.contains(scope.as_str()));
    }
}
