use super::super::super::*;
use super::super::guards::{
    ActiveConnectionGuard, ActiveHttp3ConnectionGuard, ActiveHttp3RequestStreamGuard,
};

impl SharedState {
    pub fn active_connection_count(&self) -> usize {
        self.active_connections.load(Ordering::Acquire)
    }

    pub fn try_acquire_connection(
        &self,
        listener_id: &str,
        limit: Option<usize>,
    ) -> Option<ActiveConnectionGuard> {
        let listener_active_connections = self
            .listener_active_connections
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(listener_id)?
            .clone();
        loop {
            let current = listener_active_connections.load(Ordering::Acquire);
            if limit.is_some_and(|limit| current >= limit) {
                return None;
            }

            if listener_active_connections
                .compare_exchange_weak(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                self.active_connections.fetch_add(1, Ordering::AcqRel);
                return Some(ActiveConnectionGuard {
                    active_connections: self.active_connections.clone(),
                    listener_active_connections,
                    listener_id: listener_id.to_string(),
                    snapshot_version: self.snapshot_version.clone(),
                    snapshot_notify: self.snapshot_notify.clone(),
                    snapshot_components: self.snapshot_components.clone(),
                    traffic_component_versions: self.traffic_component_versions.clone(),
                });
            }
        }
    }

    pub fn retain_connection_slot(&self, listener_id: &str) -> ActiveConnectionGuard {
        let listener_active_connections = self
            .listener_active_connections
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(listener_id)
            .expect("listener id should exist while retaining a connection slot")
            .clone();
        listener_active_connections.fetch_add(1, Ordering::AcqRel);
        self.active_connections.fetch_add(1, Ordering::AcqRel);
        let version = self.mark_snapshot_changed_components(true, false, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
        self.notify_snapshot_waiters();
        ActiveConnectionGuard {
            active_connections: self.active_connections.clone(),
            listener_active_connections,
            listener_id: listener_id.to_string(),
            snapshot_version: self.snapshot_version.clone(),
            snapshot_notify: self.snapshot_notify.clone(),
            snapshot_components: self.snapshot_components.clone(),
            traffic_component_versions: self.traffic_component_versions.clone(),
        }
    }

    pub(crate) fn retain_http3_connection(
        &self,
        listener_id: &str,
    ) -> Result<ActiveHttp3ConnectionGuard> {
        let counters = self.listener_traffic_counters(listener_id).ok_or_else(|| {
            rginx_core::Error::Server(format!(
                "listener `{listener_id}` is missing traffic counters for http3 connections"
            ))
        })?;
        counters.active_http3_connections.fetch_add(1, Ordering::AcqRel);
        let version = self.mark_snapshot_changed_components(true, false, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
        self.notify_snapshot_waiters();
        Ok(ActiveHttp3ConnectionGuard {
            counters,
            listener_id: listener_id.to_string(),
            snapshot_version: self.snapshot_version.clone(),
            snapshot_notify: self.snapshot_notify.clone(),
            snapshot_components: self.snapshot_components.clone(),
            traffic_component_versions: self.traffic_component_versions.clone(),
        })
    }

    pub(crate) fn retain_http3_request_stream(
        &self,
        listener_id: &str,
    ) -> Result<ActiveHttp3RequestStreamGuard> {
        let counters = self.listener_traffic_counters(listener_id).ok_or_else(|| {
            rginx_core::Error::Server(format!(
                "listener `{listener_id}` is missing traffic counters for http3 request streams"
            ))
        })?;
        counters.active_http3_request_streams.fetch_add(1, Ordering::AcqRel);
        let version = self.mark_snapshot_changed_components(true, false, true, false, false);
        self.mark_traffic_targets_changed(version, Some(listener_id), None, None);
        self.notify_snapshot_waiters();
        Ok(ActiveHttp3RequestStreamGuard {
            counters,
            listener_id: listener_id.to_string(),
            snapshot_version: self.snapshot_version.clone(),
            snapshot_notify: self.snapshot_notify.clone(),
            snapshot_components: self.snapshot_components.clone(),
            traffic_component_versions: self.traffic_component_versions.clone(),
        })
    }
}
