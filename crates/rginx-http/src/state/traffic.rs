use super::*;

struct TrafficCounterRefs {
    listener: Option<Arc<ListenerTrafficCounters>>,
    vhost: Option<Arc<RequestTrafficCounters>>,
    route: Option<Arc<RouteTrafficCounters>>,
}

impl SharedState {
    pub fn counters_snapshot(&self) -> HttpCountersSnapshot {
        self.counters.snapshot()
    }

    pub fn traffic_stats_snapshot(&self) -> TrafficStatsSnapshot {
        self.traffic_stats_snapshot_with_window(None)
    }

    pub fn traffic_stats_snapshot_with_window(
        &self,
        window_secs: Option<u64>,
    ) -> TrafficStatsSnapshot {
        let stats = self.traffic_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());

        let listeners = stats
            .listener_order
            .iter()
            .filter_map(|listener_id| {
                let entry = stats.listeners.get(listener_id)?;
                let (
                    downstream_responses,
                    downstream_responses_1xx,
                    downstream_responses_2xx,
                    downstream_responses_3xx,
                    downstream_responses_4xx,
                    downstream_responses_5xx,
                ) = entry.counters.responses.snapshot();
                Some(ListenerStatsSnapshot {
                    listener_id: listener_id.clone(),
                    listener_name: entry.listener_name.clone(),
                    listen_addr: entry.listen_addr,
                    active_connections: self
                        .listener_active_connections
                        .read()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .get(listener_id)
                        .map(|connections| connections.load(Ordering::Acquire))
                        .unwrap_or(0),
                    downstream_connections_accepted: entry
                        .counters
                        .downstream_connections_accepted
                        .load(Ordering::Relaxed),
                    downstream_connections_rejected: entry
                        .counters
                        .downstream_connections_rejected
                        .load(Ordering::Relaxed),
                    downstream_requests: entry.counters.downstream_requests.load(Ordering::Relaxed),
                    unmatched_requests_total: entry
                        .counters
                        .unmatched_requests_total
                        .load(Ordering::Relaxed),
                    downstream_responses,
                    downstream_responses_1xx,
                    downstream_responses_2xx,
                    downstream_responses_3xx,
                    downstream_responses_4xx,
                    downstream_responses_5xx,
                    recent_60s: entry.counters.recent_60s.snapshot(),
                    recent_window: window_secs.map(|window_secs| {
                        entry.counters.recent_60s.snapshot_for_window(window_secs)
                    }),
                    grpc: entry.counters.grpc.snapshot(),
                })
            })
            .collect();

        let vhosts = stats
            .vhost_order
            .iter()
            .filter_map(|vhost_id| {
                let entry = stats.vhosts.get(vhost_id)?;
                let (
                    downstream_responses,
                    downstream_responses_1xx,
                    downstream_responses_2xx,
                    downstream_responses_3xx,
                    downstream_responses_4xx,
                    downstream_responses_5xx,
                ) = entry.counters.responses.snapshot();
                Some(VhostStatsSnapshot {
                    vhost_id: vhost_id.clone(),
                    server_names: entry.server_names.clone(),
                    downstream_requests: entry.counters.downstream_requests.load(Ordering::Relaxed),
                    unmatched_requests_total: entry
                        .counters
                        .unmatched_requests_total
                        .load(Ordering::Relaxed),
                    downstream_responses,
                    downstream_responses_1xx,
                    downstream_responses_2xx,
                    downstream_responses_3xx,
                    downstream_responses_4xx,
                    downstream_responses_5xx,
                    recent_60s: entry.counters.recent_60s.snapshot(),
                    recent_window: window_secs.map(|window_secs| {
                        entry.counters.recent_60s.snapshot_for_window(window_secs)
                    }),
                    grpc: entry.counters.grpc.snapshot(),
                })
            })
            .collect();

        let routes = stats
            .route_order
            .iter()
            .filter_map(|route_id| {
                let entry = stats.routes.get(route_id)?;
                let (
                    downstream_responses,
                    downstream_responses_1xx,
                    downstream_responses_2xx,
                    downstream_responses_3xx,
                    downstream_responses_4xx,
                    downstream_responses_5xx,
                ) = entry.counters.responses.snapshot();
                Some(RouteStatsSnapshot {
                    route_id: route_id.clone(),
                    vhost_id: entry.vhost_id.clone(),
                    downstream_requests: entry.counters.downstream_requests.load(Ordering::Relaxed),
                    downstream_responses,
                    downstream_responses_1xx,
                    downstream_responses_2xx,
                    downstream_responses_3xx,
                    downstream_responses_4xx,
                    downstream_responses_5xx,
                    access_denied_total: entry.counters.access_denied_total.load(Ordering::Relaxed),
                    rate_limited_total: entry.counters.rate_limited_total.load(Ordering::Relaxed),
                    recent_60s: entry.counters.recent_60s.snapshot(),
                    recent_window: window_secs.map(|window_secs| {
                        entry.counters.recent_60s.snapshot_for_window(window_secs)
                    }),
                    grpc: entry.counters.grpc.snapshot(),
                })
            })
            .collect();

        TrafficStatsSnapshot { listeners, vhosts, routes }
    }

    pub(crate) fn record_downstream_request(
        &self,
        listener_id: &str,
        vhost_id: &str,
        route_id: Option<&str>,
    ) {
        let counters = self.traffic_counter_refs(listener_id, vhost_id, route_id);
        self.counters.downstream_requests.fetch_add(1, Ordering::Relaxed);
        if let Some(listener) = counters.listener {
            listener.downstream_requests.fetch_add(1, Ordering::Relaxed);
            listener.recent_60s.record_downstream_request();
            if route_id.is_none() {
                listener.unmatched_requests_total.fetch_add(1, Ordering::Relaxed);
            }
        }
        if let Some(vhost) = counters.vhost {
            vhost.downstream_requests.fetch_add(1, Ordering::Relaxed);
            vhost.recent_60s.record_downstream_request();
            if route_id.is_none() {
                vhost.unmatched_requests_total.fetch_add(1, Ordering::Relaxed);
            }
        }
        if let Some(route) = counters.route {
            route.downstream_requests.fetch_add(1, Ordering::Relaxed);
            route.recent_60s.record_downstream_request();
        }
        let version = self.mark_snapshot_changed_components(false, true, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), Some(vhost_id), route_id);
        self.notify_snapshot_waiters();
    }

    pub(crate) fn record_mtls_request(&self, listener_id: &str, authenticated: bool) {
        if authenticated {
            self.counters.downstream_mtls_authenticated_requests.fetch_add(1, Ordering::Relaxed);
        } else {
            self.counters.downstream_mtls_anonymous_requests.fetch_add(1, Ordering::Relaxed);
        }

        if let Some(counters) = self.listener_traffic_counters(listener_id) {
            if authenticated {
                counters.downstream_mtls_authenticated_requests.fetch_add(1, Ordering::Relaxed);
            } else {
                counters.downstream_mtls_anonymous_requests.fetch_add(1, Ordering::Relaxed);
            }
        }

        let version = self.mark_snapshot_changed_components(true, true, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
        self.notify_snapshot_waiters();
    }

    pub(crate) fn record_downstream_response(
        &self,
        listener_id: &str,
        vhost_id: &str,
        route_id: Option<&str>,
        status: StatusCode,
    ) {
        let counters = self.traffic_counter_refs(listener_id, vhost_id, route_id);
        self.counters.downstream_responses.fetch_add(1, Ordering::Relaxed);
        match status.as_u16() / 100 {
            1 => {
                self.counters.downstream_responses_1xx.fetch_add(1, Ordering::Relaxed);
            }
            2 => {
                self.counters.downstream_responses_2xx.fetch_add(1, Ordering::Relaxed);
            }
            3 => {
                self.counters.downstream_responses_3xx.fetch_add(1, Ordering::Relaxed);
            }
            4 => {
                self.counters.downstream_responses_4xx.fetch_add(1, Ordering::Relaxed);
            }
            5 => {
                self.counters.downstream_responses_5xx.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
        if let Some(listener) = counters.listener {
            listener.responses.record(status);
            listener.recent_60s.record_downstream_response(status);
        }
        if let Some(vhost) = counters.vhost {
            vhost.responses.record(status);
            vhost.recent_60s.record_downstream_response(status);
        }
        if let Some(route) = counters.route {
            route.responses.record(status);
            route.recent_60s.record_downstream_response(status);
        }
        let version = self.mark_snapshot_changed_components(false, true, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), Some(vhost_id), route_id);
        self.notify_snapshot_waiters();
    }

    pub(crate) fn record_route_access_denied(&self, route_id: &str) {
        if let Some(counters) = self.route_traffic_counters(route_id) {
            counters.access_denied_total.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(false, false, true, false, false);
        self.mark_traffic_targets_changed(version, None, None, Some(route_id));
        self.notify_snapshot_waiters();
    }

    pub(crate) fn record_route_rate_limited(&self, route_id: &str) {
        if let Some(counters) = self.route_traffic_counters(route_id) {
            counters.rate_limited_total.fetch_add(1, Ordering::Relaxed);
        }
        let version = self.mark_snapshot_changed_components(false, false, true, false, false);
        self.mark_traffic_targets_changed(version, None, None, Some(route_id));
        self.notify_snapshot_waiters();
    }

    pub(crate) fn record_grpc_request(
        &self,
        listener_id: &str,
        vhost_id: &str,
        route_id: Option<&str>,
        protocol: &str,
    ) {
        let counters = self.traffic_counter_refs(listener_id, vhost_id, route_id);
        if let Some(listener) = counters.listener {
            listener.grpc.record_request(protocol);
            listener.recent_60s.record_grpc_request();
        }
        if let Some(vhost) = counters.vhost {
            vhost.grpc.record_request(protocol);
            vhost.recent_60s.record_grpc_request();
        }
        if let Some(route) = counters.route {
            route.grpc.record_request(protocol);
            route.recent_60s.record_grpc_request();
        }
        let version = self.mark_snapshot_changed_components(false, false, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), Some(vhost_id), route_id);
        self.notify_snapshot_waiters();
    }

    pub(crate) fn record_grpc_status(
        &self,
        listener_id: &str,
        vhost_id: &str,
        route_id: Option<&str>,
        status: Option<&str>,
    ) {
        let counters = self.traffic_counter_refs(listener_id, vhost_id, route_id);
        if let Some(listener) = counters.listener {
            listener.grpc.record_status(status);
        }
        if let Some(vhost) = counters.vhost {
            vhost.grpc.record_status(status);
        }
        if let Some(route) = counters.route {
            route.grpc.record_status(status);
        }
        let version = self.mark_snapshot_changed_components(false, false, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), Some(vhost_id), route_id);
        self.notify_snapshot_waiters();
    }

    pub(crate) fn sync_traffic_stats(&self, config: &ConfigSnapshot) {
        let existing = self.traffic_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let next = build_traffic_stats_index(config, Some(&*existing));
        drop(existing);
        *self.traffic_stats.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
        let existing =
            self.traffic_component_versions.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let next = build_traffic_component_versions(config, Some(&*existing));
        drop(existing);
        *self.traffic_component_versions.write().unwrap_or_else(|poisoned| poisoned.into_inner()) =
            next;
    }

    pub(super) fn listener_traffic_counters(
        &self,
        listener_id: &str,
    ) -> Option<Arc<ListenerTrafficCounters>> {
        let stats = self.traffic_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        stats.listeners.get(listener_id).map(|entry| entry.counters.clone())
    }

    fn route_traffic_counters(&self, route_id: &str) -> Option<Arc<RouteTrafficCounters>> {
        let stats = self.traffic_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        stats.routes.get(route_id).map(|entry| entry.counters.clone())
    }

    fn traffic_counter_refs(
        &self,
        listener_id: &str,
        vhost_id: &str,
        route_id: Option<&str>,
    ) -> TrafficCounterRefs {
        let stats = self.traffic_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        TrafficCounterRefs {
            listener: stats.listeners.get(listener_id).map(|entry| entry.counters.clone()),
            vhost: stats.vhosts.get(vhost_id).map(|entry| entry.counters.clone()),
            route: route_id.and_then(|route_id| {
                stats.routes.get(route_id).map(|entry| entry.counters.clone())
            }),
        }
    }

    pub(crate) fn changed_traffic_targets_since(
        &self,
        since_version: u64,
    ) -> (Vec<String>, Vec<String>, Vec<String>) {
        let versions =
            self.traffic_component_versions.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut listeners = versions
            .listeners
            .iter()
            .filter_map(|(id, version)| (*version > since_version).then_some(id.clone()))
            .collect::<Vec<_>>();
        listeners.sort();
        let mut vhosts = versions
            .vhosts
            .iter()
            .filter_map(|(id, version)| (*version > since_version).then_some(id.clone()))
            .collect::<Vec<_>>();
        vhosts.sort();
        let mut routes = versions
            .routes
            .iter()
            .filter_map(|(id, version)| (*version > since_version).then_some(id.clone()))
            .collect::<Vec<_>>();
        routes.sort();
        (listeners, vhosts, routes)
    }

    pub(crate) fn mark_traffic_targets_changed(
        &self,
        version: u64,
        listener_id: Option<&str>,
        vhost_id: Option<&str>,
        route_id: Option<&str>,
    ) {
        let mut versions = self
            .traffic_component_versions
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(listener_id) = listener_id {
            versions.listeners.insert(listener_id.to_string(), version);
        }
        if let Some(vhost_id) = vhost_id {
            versions.vhosts.insert(vhost_id.to_string(), version);
        }
        if let Some(route_id) = route_id {
            versions.routes.insert(route_id.to_string(), version);
        }
    }
}
