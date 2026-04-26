use super::super::*;

impl SharedState {
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
}
