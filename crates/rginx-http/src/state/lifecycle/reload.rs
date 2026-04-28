use super::super::*;
use super::topology::{traffic_topology_changed, upstream_topology_changed};
use crate::validate_config_transition;

impl SharedState {
    pub fn record_reload_success(&self, revision: u64, tls_certificate_changes: Vec<String>) {
        let mut history =
            self.reload_history.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        history.attempts_total += 1;
        history.successes_total += 1;
        history.last_result = Some(ReloadResultSnapshot {
            finished_at_unix_ms: unix_time_ms(SystemTime::now()),
            outcome: ReloadOutcomeSnapshot::Success { revision },
            tls_certificate_changes,
            active_revision: revision,
            rollback_preserved_revision: None,
        });
        self.mark_snapshot_changed_components(true, false, false, false, false);
        self.notify_snapshot_waiters();
    }

    pub fn record_reload_failure(&self, error: impl Into<String>, active_revision: u64) {
        let mut history =
            self.reload_history.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        history.attempts_total += 1;
        history.failures_total += 1;
        history.last_result = Some(ReloadResultSnapshot {
            finished_at_unix_ms: unix_time_ms(SystemTime::now()),
            outcome: ReloadOutcomeSnapshot::Failure { error: error.into() },
            tls_certificate_changes: Vec::new(),
            active_revision,
            rollback_preserved_revision: Some(active_revision),
        });
        self.mark_snapshot_changed_components(true, false, false, false, false);
        self.notify_snapshot_waiters();
    }

    pub fn record_ocsp_refresh_success(&self, scope: &str) {
        let mut statuses =
            self.ocsp_statuses.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = statuses.entry(scope.to_string()).or_default();
        entry.last_refresh_unix_ms = Some(unix_time_ms(SystemTime::now()));
        entry.refreshes_total += 1;
        entry.last_error = None;
        self.mark_snapshot_changed_components(true, false, false, false, false);
        self.notify_snapshot_waiters();
    }

    pub fn record_ocsp_refresh_failure(&self, scope: &str, error: impl Into<String>) {
        let mut statuses =
            self.ocsp_statuses.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = statuses.entry(scope.to_string()).or_default();
        entry.failures_total += 1;
        entry.last_error = Some(error.into());
        self.mark_snapshot_changed_components(true, false, false, false, false);
        self.notify_snapshot_waiters();
    }

    pub async fn replace(&self, config: ConfigSnapshot) -> Result<Arc<ConfigSnapshot>> {
        let prepared = self.prepare_replacement(config).await?;
        Ok(self.commit_prepared(prepared).await)
    }

    pub async fn refresh_tls_acceptors_from_current_config(&self) -> Result<()> {
        let config = self.current_config().await;
        let listener_tls_acceptors = prepare_listener_tls_acceptors(config.as_ref())?;
        *self.listener_tls_acceptors.write().await = listener_tls_acceptors;
        self.mark_snapshot_changed_components(true, false, false, false, false);
        self.notify_snapshot_waiters();
        Ok(())
    }

    async fn prepare_replacement(&self, config: ConfigSnapshot) -> Result<PreparedState> {
        let current = self.current_config().await;
        validate_config_transition(current.as_ref(), &config)?;
        let mut prepared = prepare_state(
            config,
            Some(build_peer_health_notifier(
                self.snapshot_version.clone(),
                self.snapshot_notify.clone(),
                self.snapshot_components.clone(),
                self.peer_health_component_versions.clone(),
            )),
            Some(build_cache_notifier(
                self.snapshot_version.clone(),
                self.snapshot_notify.clone(),
                self.snapshot_components.clone(),
                self.cache_component_versions.clone(),
            )),
        )?;
        prepared.retired_listeners = current
            .listeners
            .iter()
            .filter(|listener| prepared.config.listener(&listener.id).is_none())
            .cloned()
            .collect();
        Ok(prepared)
    }

    async fn commit_prepared(&self, prepared: PreparedState) -> Arc<ConfigSnapshot> {
        let previous_config = self.current_config().await;
        if !prepared.retired_listeners.is_empty() {
            let mut retired =
                self.retired_listeners.write().unwrap_or_else(|poisoned| poisoned.into_inner());
            for listener in &prepared.retired_listeners {
                retired.insert(listener.id.clone(), listener.clone());
            }
        }

        let existing_acceptors = self.listener_tls_acceptors.read().await.clone();
        let mut merged_acceptors = prepared.listener_tls_acceptors.clone();
        for listener in &prepared.retired_listeners {
            if let Some(acceptor) = existing_acceptors.get(&listener.id) {
                merged_acceptors.insert(listener.id.clone(), acceptor.clone());
            }
        }

        self.sync_listener_active_connections(prepared.config.as_ref());
        self.sync_traffic_stats(prepared.config.as_ref());
        self.sync_peer_health_versions(prepared.config.as_ref());
        self.sync_upstream_stats(prepared.config.as_ref());
        self.sync_cache_versions(prepared.config.as_ref());

        let next_revision = {
            let mut state = self.inner.write().await;
            let next_revision = state.revision + 1;
            state.revision = next_revision;
            state.config = prepared.config.clone();
            state.clients = prepared.clients;
            state.cache = prepared.cache;
            next_revision
        };

        *self.listener_tls_acceptors.write().await = merged_acceptors;
        let _ = self.revisions.send(next_revision);
        self.mark_snapshot_changed_components_with_cache(true, false, true, true, true, true);
        if traffic_topology_changed(previous_config.as_ref(), prepared.config.as_ref()) {
            self.mark_all_traffic_targets_changed(
                previous_config.as_ref(),
                prepared.config.as_ref(),
                next_revision,
            );
        }
        if upstream_topology_changed(previous_config.as_ref(), prepared.config.as_ref()) {
            self.mark_all_upstream_targets_changed(
                previous_config.as_ref(),
                prepared.config.as_ref(),
                next_revision,
            );
        }
        self.mark_all_cache_zones_changed(
            previous_config.as_ref(),
            prepared.config.as_ref(),
            next_revision,
        );
        self.notify_snapshot_waiters();

        prepared.config
    }
}
