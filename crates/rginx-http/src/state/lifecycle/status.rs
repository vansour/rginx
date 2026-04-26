use super::super::tls_runtime::{
    tls_runtime_snapshot_for_config_with_ocsp_statuses, upstream_tls_status_snapshots,
};
use super::super::*;

impl SharedState {
    pub fn rate_limiters(&self) -> RateLimiters {
        self.rate_limiters.clone()
    }

    pub fn reload_status_snapshot(&self) -> ReloadStatusSnapshot {
        self.reload_history.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).snapshot()
    }

    pub async fn status_snapshot(&self) -> RuntimeStatusSnapshot {
        let (revision, config) = {
            let state = self.inner.read().await;
            (state.revision, state.config.clone())
        };
        let ocsp_statuses =
            self.ocsp_statuses.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let mtls = self.mtls_status_snapshot(config.as_ref());
        let tls = tls_runtime_snapshot_for_config_with_ocsp_statuses(
            config.as_ref(),
            Some(&ocsp_statuses),
        );
        let listener_traffic =
            self.traffic_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut http3_active_connections = 0;
        let mut http3_active_request_streams = 0;
        let mut http3_retry_issued_total = 0;
        let mut http3_retry_failed_total = 0;
        let mut http3_request_accept_errors_total = 0;
        let mut http3_request_resolve_errors_total = 0;
        let mut http3_request_body_stream_errors_total = 0;
        let mut http3_response_stream_errors_total = 0;
        for listener in &config.listeners {
            let Some(entry) = listener_traffic.listeners.get(&listener.id) else {
                continue;
            };
            let counters = &entry.counters;
            http3_active_connections += counters.active_http3_connections.load(Ordering::Acquire);
            http3_active_request_streams +=
                counters.active_http3_request_streams.load(Ordering::Acquire);
            http3_retry_issued_total += counters.http3_retry_issued_total.load(Ordering::Relaxed);
            http3_retry_failed_total += counters.http3_retry_failed_total.load(Ordering::Relaxed);
            http3_request_accept_errors_total +=
                counters.http3_request_accept_errors_total.load(Ordering::Relaxed);
            http3_request_resolve_errors_total +=
                counters.http3_request_resolve_errors_total.load(Ordering::Relaxed);
            http3_request_body_stream_errors_total +=
                counters.http3_request_body_stream_errors_total.load(Ordering::Relaxed);
            http3_response_stream_errors_total +=
                counters.http3_response_stream_errors_total.load(Ordering::Relaxed);
        }
        RuntimeStatusSnapshot {
            revision,
            config_path: self.config_path.as_deref().cloned(),
            listeners: config
                .listeners
                .iter()
                .map(|listener| {
                    let listener_entry = listener_traffic.listeners.get(&listener.id);
                    let listener_counters = listener_entry.map(|entry| entry.counters.as_ref());
                    RuntimeListenerSnapshot {
                        listener_id: listener.id.clone(),
                        listener_name: listener.name.clone(),
                        listen_addr: listener.server.listen_addr,
                        binding_count: listener.binding_count(),
                        http3_enabled: listener.http3_enabled(),
                        tls_enabled: listener.tls_enabled(),
                        proxy_protocol_enabled: listener.proxy_protocol_enabled,
                        default_certificate: listener.server.default_certificate.clone(),
                        keep_alive: listener.server.keep_alive,
                        max_connections: listener.server.max_connections,
                        access_log_format_configured: listener.server.access_log_format.is_some(),
                        http3_runtime: http3_runtime_snapshot(
                            listener.http3.is_some(),
                            listener_counters,
                        ),
                        bindings: listener
                            .transport_bindings()
                            .into_iter()
                            .map(|binding| crate::state::RuntimeListenerBindingSnapshot {
                                advertise_alt_svc: (binding.kind
                                    == rginx_core::ListenerTransportKind::Udp)
                                    .then_some(binding.advertise_alt_svc),
                                alt_svc_max_age_secs: if binding.kind
                                    == rginx_core::ListenerTransportKind::Udp
                                {
                                    binding.alt_svc_max_age.map(|max_age| max_age.as_secs())
                                } else {
                                    None
                                },
                                binding_name: binding.name.to_string(),
                                transport: binding.kind.as_str().to_string(),
                                listen_addr: binding.listen_addr,
                                protocols: binding
                                    .protocols
                                    .into_iter()
                                    .map(|protocol| protocol.as_str().to_string())
                                    .collect(),
                                worker_count: config.runtime.accept_workers,
                                reuse_port_enabled: (binding.kind
                                    == rginx_core::ListenerTransportKind::Udp)
                                    .then_some(config.runtime.accept_workers > 1),
                                http3_max_concurrent_streams: binding.http3_max_concurrent_streams,
                                http3_stream_buffer_size: binding.http3_stream_buffer_size,
                                http3_active_connection_id_limit: binding
                                    .http3_active_connection_id_limit,
                                http3_retry: binding.http3_retry,
                                http3_host_key_path: binding.http3_host_key_path.clone(),
                                http3_gso: binding.http3_gso,
                                http3_early_data_enabled: binding.http3_early_data_enabled,
                            })
                            .collect(),
                    }
                })
                .collect(),
            worker_threads: config.runtime.worker_threads,
            accept_workers: config.runtime.accept_workers,
            total_vhosts: config.total_vhost_count(),
            total_routes: config.total_route_count(),
            total_upstreams: config.upstreams.len(),
            tls_enabled: config.tls_enabled(),
            http3_active_connections,
            http3_active_request_streams,
            http3_retry_issued_total,
            http3_retry_failed_total,
            http3_request_accept_errors_total,
            http3_request_resolve_errors_total,
            http3_request_body_stream_errors_total,
            http3_response_stream_errors_total,
            http3_early_data_enabled_listeners: config
                .listeners
                .iter()
                .filter(|listener| {
                    listener.http3.as_ref().is_some_and(|http3| http3.early_data_enabled)
                })
                .count(),
            http3_early_data_accepted_requests: self
                .counters
                .downstream_http3_early_data_accepted_requests
                .load(Ordering::Relaxed),
            http3_early_data_rejected_requests: self
                .counters
                .downstream_http3_early_data_rejected_requests
                .load(Ordering::Relaxed),
            tls,
            mtls,
            upstream_tls: upstream_tls_status_snapshots(config.as_ref()),
            active_connections: self.active_connection_count(),
            reload: self.reload_status_snapshot(),
        }
    }

    pub async fn tls_acceptor(&self, listener_id: &str) -> Option<TlsAcceptor> {
        self.listener_tls_acceptors.read().await.get(listener_id).cloned().flatten()
    }

    pub fn next_request_id(&self) -> String {
        uuid::Uuid::now_v7().to_string()
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

    pub(super) fn sync_listener_active_connections(&self, config: &ConfigSnapshot) {
        let mut active = self
            .listener_active_connections
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for listener in &config.listeners {
            active.entry(listener.id.clone()).or_insert_with(|| Arc::new(AtomicUsize::new(0)));
        }
    }
}
