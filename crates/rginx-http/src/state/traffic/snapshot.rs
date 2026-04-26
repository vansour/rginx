use super::super::*;

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
                    http3_runtime: http3_runtime_snapshot(
                        entry.http3_enabled,
                        Some(entry.counters.as_ref()),
                    ),
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

    pub(in crate::state) fn listener_traffic_counters(
        &self,
        listener_id: &str,
    ) -> Option<Arc<ListenerTrafficCounters>> {
        let stats = self.traffic_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        stats.listeners.get(listener_id).map(|entry| entry.counters.clone())
    }

    pub(super) fn route_traffic_counters(
        &self,
        route_id: &str,
    ) -> Option<Arc<RouteTrafficCounters>> {
        let stats = self.traffic_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        stats.routes.get(route_id).map(|entry| entry.counters.clone())
    }

    pub(super) fn traffic_counter_refs(
        &self,
        listener_id: &str,
        vhost_id: &str,
        route_id: Option<&str>,
    ) -> super::TrafficCounterRefs {
        let stats = self.traffic_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        super::TrafficCounterRefs {
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

    pub(crate) fn mark_all_traffic_targets_changed(
        &self,
        previous: &ConfigSnapshot,
        next: &ConfigSnapshot,
        version: u64,
    ) {
        let mut versions = self
            .traffic_component_versions
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        for listener in &previous.listeners {
            versions.listeners.insert(listener.id.clone(), version);
        }
        for listener in &next.listeners {
            versions.listeners.insert(listener.id.clone(), version);
        }

        for vhost in std::iter::once(&previous.default_vhost).chain(previous.vhosts.iter()) {
            versions.vhosts.insert(vhost.id.clone(), version);
            for route in &vhost.routes {
                versions.routes.insert(route.id.clone(), version);
            }
        }
        for vhost in std::iter::once(&next.default_vhost).chain(next.vhosts.iter()) {
            versions.vhosts.insert(vhost.id.clone(), version);
            for route in &vhost.routes {
                versions.routes.insert(route.id.clone(), version);
            }
        }
    }
}
