use super::tls_runtime::{
    tls_runtime_snapshot_for_config_with_ocsp_statuses, upstream_tls_status_snapshots,
};
use super::*;
use crate::validate_config_transition;

impl SharedState {
    pub fn rate_limiters(&self) -> RateLimiters {
        self.rate_limiters.clone()
    }

    pub fn reload_status_snapshot(&self) -> ReloadStatusSnapshot {
        self.reload_history.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).snapshot()
    }

    pub async fn status_snapshot(&self) -> RuntimeStatusSnapshot {
        let state = self.inner.read().await;
        let mtls = self.mtls_status_snapshot(state.config.as_ref());
        let ocsp_statuses =
            self.ocsp_statuses.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let tls = tls_runtime_snapshot_for_config_with_ocsp_statuses(
            state.config.as_ref(),
            Some(&ocsp_statuses),
        );
        RuntimeStatusSnapshot {
            revision: state.revision,
            config_path: self.config_path.as_deref().cloned(),
            listen_addr: state.config.server.listen_addr,
            worker_threads: state.config.runtime.worker_threads,
            accept_workers: state.config.runtime.accept_workers,
            total_vhosts: state.config.total_vhost_count(),
            total_routes: state.config.total_route_count(),
            total_upstreams: state.config.upstreams.len(),
            tls_enabled: state.config.tls_enabled(),
            tls,
            mtls,
            upstream_tls: upstream_tls_status_snapshots(state.config.as_ref()),
            active_connections: self.active_connection_count(),
            reload: self.reload_status_snapshot(),
        }
    }

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
    }

    pub fn record_ocsp_refresh_success(&self, scope: &str) {
        let mut statuses =
            self.ocsp_statuses.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = statuses.entry(scope.to_string()).or_default();
        entry.last_refresh_unix_ms = Some(unix_time_ms(SystemTime::now()));
        entry.refreshes_total += 1;
        entry.last_error = None;
        self.mark_snapshot_changed_components(true, false, false, false, false);
    }

    pub fn record_ocsp_refresh_failure(&self, scope: &str, error: impl Into<String>) {
        let mut statuses =
            self.ocsp_statuses.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = statuses.entry(scope.to_string()).or_default();
        entry.failures_total += 1;
        entry.last_error = Some(error.into());
        self.mark_snapshot_changed_components(true, false, false, false, false);
    }

    pub async fn tls_acceptor(&self, listener_id: &str) -> Option<TlsAcceptor> {
        self.listener_tls_acceptors.read().await.get(listener_id).cloned().flatten()
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

        let next_revision = {
            let mut state = self.inner.write().await;
            let next_revision = state.revision + 1;
            state.revision = next_revision;
            state.config = prepared.config.clone();
            state.clients = prepared.clients;
            next_revision
        };

        *self.listener_tls_acceptors.write().await = merged_acceptors;
        let _ = self.revisions.send(next_revision);
        self.mark_snapshot_changed_components(true, false, true, true, true);

        prepared.config
    }

    pub fn spawn_background_task<F>(&self, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let handle = tokio::spawn(task);
        let mut tasks =
            self.background_tasks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        tasks.retain(|task| !task.is_finished());
        tasks.push(handle);
    }

    pub fn next_request_id(&self) -> String {
        let next = self.request_ids.fetch_add(1, Ordering::Relaxed);
        format!("rginx-{next:016x}")
    }

    fn mtls_status_snapshot(&self, config: &ConfigSnapshot) -> MtlsStatusSnapshot {
        let mut configured_listeners = 0usize;
        let mut optional_listeners = 0usize;
        let mut required_listeners = 0usize;
        let mut authenticated_connections = 0u64;
        let mut authenticated_requests = 0u64;
        let mut anonymous_requests = 0u64;
        let mut handshake_failures_total = 0u64;

        for listener in &config.listeners {
            let Some(client_auth) =
                listener.server.tls.as_ref().and_then(|tls| tls.client_auth.as_ref())
            else {
                continue;
            };
            configured_listeners += 1;
            match client_auth.mode {
                rginx_core::ServerClientAuthMode::Optional => optional_listeners += 1,
                rginx_core::ServerClientAuthMode::Required => required_listeners += 1,
            }

            if let Some(counters) = self.listener_traffic_counters(&listener.id) {
                authenticated_connections +=
                    counters.downstream_mtls_authenticated_connections.load(Ordering::Relaxed);
                authenticated_requests +=
                    counters.downstream_mtls_authenticated_requests.load(Ordering::Relaxed);
                anonymous_requests +=
                    counters.downstream_mtls_anonymous_requests.load(Ordering::Relaxed);
                handshake_failures_total +=
                    counters.downstream_tls_handshake_failures.load(Ordering::Relaxed);
            }
        }

        let counters = self.counters_snapshot();
        MtlsStatusSnapshot {
            configured_listeners,
            optional_listeners,
            required_listeners,
            authenticated_connections,
            authenticated_requests,
            anonymous_requests,
            handshake_failures_total,
            handshake_failures_missing_client_cert: counters
                .downstream_tls_handshake_failures_missing_client_cert,
            handshake_failures_unknown_ca: counters.downstream_tls_handshake_failures_unknown_ca,
            handshake_failures_bad_certificate: counters
                .downstream_tls_handshake_failures_bad_certificate,
            handshake_failures_certificate_revoked: counters
                .downstream_tls_handshake_failures_certificate_revoked,
            handshake_failures_verify_depth_exceeded: counters
                .downstream_tls_handshake_failures_verify_depth_exceeded,
            handshake_failures_other: counters.downstream_tls_handshake_failures_other,
        }
    }

    pub async fn drain_background_tasks(&self) {
        for task in take_background_tasks(&self.background_tasks) {
            if let Err(error) = task.await {
                if error.is_panic() {
                    tracing::warn!(%error, "background task panicked");
                } else if !error.is_cancelled() {
                    tracing::warn!(%error, "background task failed to join");
                }
            }
        }
    }

    pub async fn abort_background_tasks(&self) {
        let tasks = take_background_tasks(&self.background_tasks);
        for task in &tasks {
            task.abort();
        }

        for task in tasks {
            if let Err(error) = task.await
                && !error.is_cancelled()
            {
                tracing::warn!(%error, "background task failed after abort");
            }
        }
    }

    pub fn retire_listener_runtime(&self, listener: &Listener) {
        self.retired_listeners
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(listener.id.clone(), listener.clone());
        self.listener_active_connections
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(listener.id.clone())
            .or_insert_with(|| Arc::new(AtomicUsize::new(0)));
    }

    pub async fn remove_retired_listener_runtime(&self, listener_id: &str) {
        self.retired_listeners
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(listener_id);
        self.listener_tls_acceptors.write().await.remove(listener_id);
        self.listener_active_connections
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(listener_id);
    }

    fn sync_listener_active_connections(&self, config: &ConfigSnapshot) {
        let mut active = self
            .listener_active_connections
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for listener in &config.listeners {
            active.entry(listener.id.clone()).or_insert_with(|| Arc::new(AtomicUsize::new(0)));
        }
    }
}
