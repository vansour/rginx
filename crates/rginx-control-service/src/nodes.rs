use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use serde_json::json;

use rginx_control_store::{ControlPlaneStore, NewAuditLogEntry};
use rginx_control_types::{
    NodeAgentHeartbeatRequest, NodeAgentRegistrationRequest, NodeAgentWriteResponse,
    NodeDetailResponse, NodeLifecycleState, NodeSnapshotIngestRequest, NodeSnapshotIngestResponse,
    NodeSummary,
};

use crate::{ServiceError, ServiceResult};

static NODE_EVENT_COUNTER: AtomicU64 = AtomicU64::new(1);

const HEARTBEAT_TIMEOUT_REASON: &str = "heartbeat timeout exceeded";
const NODE_DETAIL_AUDIT_LIMIT: i64 = 12;

#[derive(Debug, Clone)]
pub struct NodeService {
    store: ControlPlaneStore,
    offline_threshold: Duration,
}

impl NodeService {
    pub fn new(store: ControlPlaneStore, offline_threshold: Duration) -> Self {
        Self { store, offline_threshold }
    }

    pub async fn register_agent(
        &self,
        request: NodeAgentRegistrationRequest,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> ServiceResult<NodeAgentWriteResponse> {
        self.validate_report(&request)?;

        let now = Utc::now();
        let details = self.build_audit_details(&request, user_agent.clone(), remote_addr.clone());
        let node = self
            .store
            .node_repository()
            .upsert_report_with_audit(
                &request,
                &NewAuditLogEntry {
                    audit_id: self.generate_id("audit"),
                    request_id: request_id.to_string(),
                    cluster_id: Some(request.cluster_id.clone()),
                    actor_id: format!("agent:{}", request.node_id),
                    action: "node.registered".to_string(),
                    resource_type: "node".to_string(),
                    resource_id: request.node_id.clone(),
                    result: "succeeded".to_string(),
                    details,
                    created_at: now,
                },
            )
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;

        Ok(NodeAgentWriteResponse { node, accepted_at_unix_ms: unix_time_ms(SystemTime::now()) })
    }

    pub async fn record_heartbeat(
        &self,
        request: NodeAgentHeartbeatRequest,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> ServiceResult<NodeAgentWriteResponse> {
        self.validate_report(&request)?;

        let now = Utc::now();
        let details = self.build_audit_details(&request, user_agent.clone(), remote_addr.clone());
        let node = self
            .store
            .node_repository()
            .upsert_report_with_audit(
                &request,
                &NewAuditLogEntry {
                    audit_id: self.generate_id("audit"),
                    request_id: request_id.to_string(),
                    cluster_id: Some(request.cluster_id.clone()),
                    actor_id: format!("agent:{}", request.node_id),
                    action: "node.heartbeat".to_string(),
                    resource_type: "node".to_string(),
                    resource_id: request.node_id.clone(),
                    result: "succeeded".to_string(),
                    details,
                    created_at: now,
                },
            )
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;

        Ok(NodeAgentWriteResponse { node, accepted_at_unix_ms: unix_time_ms(SystemTime::now()) })
    }

    pub async fn list_nodes(&self) -> ServiceResult<Vec<NodeSummary>> {
        self.store
            .node_repository()
            .list_nodes()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))
    }

    pub async fn get_node_detail(&self, node_id: &str) -> ServiceResult<NodeDetailResponse> {
        let node = self
            .store
            .node_repository()
            .load_node_summary(node_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .ok_or_else(|| ServiceError::NotFound(format!("node `{node_id}` was not found")))?;
        let latest_snapshot = self
            .store
            .node_repository()
            .load_latest_snapshot_detail(node_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        let recent_snapshots = self
            .store
            .node_repository()
            .list_recent_snapshot_metas(node_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        let recent_events = self
            .store
            .audit_repository()
            .list_recent_for_resource("node", node_id, NODE_DETAIL_AUDIT_LIMIT)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;

        Ok(NodeDetailResponse { node, latest_snapshot, recent_snapshots, recent_events })
    }

    pub async fn ingest_snapshot(
        &self,
        request: NodeSnapshotIngestRequest,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> ServiceResult<NodeSnapshotIngestResponse> {
        self.validate_snapshot(&request)?;

        let now = Utc::now();
        let snapshot = self
            .store
            .node_repository()
            .upsert_snapshot_with_audit(
                &request,
                &NewAuditLogEntry {
                    audit_id: self.generate_id("audit"),
                    request_id: request_id.to_string(),
                    cluster_id: Some(request.cluster_id.clone()),
                    actor_id: format!("agent:{}", request.node_id),
                    action: "node.snapshot_ingested".to_string(),
                    resource_type: "node".to_string(),
                    resource_id: request.node_id.clone(),
                    result: "succeeded".to_string(),
                    details: json!({
                        "snapshot_version": request.snapshot_version,
                        "schema_version": request.schema_version,
                        "captured_at_unix_ms": request.captured_at_unix_ms,
                        "binary_version": request.binary_version,
                        "included_modules": request.included_modules,
                        "user_agent": user_agent,
                        "remote_addr": remote_addr,
                    }),
                    created_at: now,
                },
            )
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;

        Ok(NodeSnapshotIngestResponse {
            snapshot,
            accepted_at_unix_ms: unix_time_ms(SystemTime::now()),
        })
    }

    pub async fn reconcile_stale_nodes(&self) -> ServiceResult<usize> {
        let observed_before = Utc::now()
            - chrono::Duration::from_std(self.offline_threshold)
                .map_err(|error| ServiceError::Internal(error.to_string()))?;
        let stale_nodes = self
            .store
            .node_repository()
            .find_stale_nodes(observed_before)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        let request_id = self.generate_id("req_worker");
        let mut updated = 0_usize;

        for node in stale_nodes {
            let result = self
                .store
                .node_repository()
                .mark_node_offline_with_audit(
                    &node.node_id,
                    observed_before,
                    HEARTBEAT_TIMEOUT_REASON,
                    &NewAuditLogEntry {
                        audit_id: self.generate_id("audit"),
                        request_id: request_id.clone(),
                        cluster_id: Some(node.cluster_id.clone()),
                        actor_id: "system:worker".to_string(),
                        action: "node.state_reconciled".to_string(),
                        resource_type: "node".to_string(),
                        resource_id: node.node_id.clone(),
                        result: "succeeded".to_string(),
                        details: json!({
                            "previous_state": node.state.as_str(),
                            "next_state": NodeLifecycleState::Offline.as_str(),
                            "reason": HEARTBEAT_TIMEOUT_REASON,
                            "offline_threshold_secs": self.offline_threshold.as_secs(),
                        }),
                        created_at: Utc::now(),
                    },
                )
                .await
                .map_err(|error| ServiceError::Internal(error.to_string()))?;

            if result.is_some() {
                updated += 1;
            }
        }

        Ok(updated)
    }

    fn validate_report(&self, request: &NodeAgentRegistrationRequest) -> ServiceResult<()> {
        if request.node_id.trim().is_empty() {
            return Err(ServiceError::BadRequest("node_id should not be empty".to_string()));
        }
        if request.cluster_id.trim().is_empty() {
            return Err(ServiceError::BadRequest("cluster_id should not be empty".to_string()));
        }
        if request.advertise_addr.trim().is_empty() {
            return Err(ServiceError::BadRequest("advertise_addr should not be empty".to_string()));
        }
        if request.role.trim().is_empty() {
            return Err(ServiceError::BadRequest("role should not be empty".to_string()));
        }
        if request.running_version.trim().is_empty() {
            return Err(ServiceError::BadRequest(
                "running_version should not be empty".to_string(),
            ));
        }
        if request.admin_socket_path.trim().is_empty() {
            return Err(ServiceError::BadRequest(
                "admin_socket_path should not be empty".to_string(),
            ));
        }

        Ok(())
    }

    fn validate_snapshot(&self, request: &NodeSnapshotIngestRequest) -> ServiceResult<()> {
        if request.node_id.trim().is_empty() {
            return Err(ServiceError::BadRequest("node_id should not be empty".to_string()));
        }
        if request.cluster_id.trim().is_empty() {
            return Err(ServiceError::BadRequest("cluster_id should not be empty".to_string()));
        }
        if request.binary_version.trim().is_empty() {
            return Err(ServiceError::BadRequest("binary_version should not be empty".to_string()));
        }
        if request.included_modules.is_empty() {
            return Err(ServiceError::BadRequest(
                "included_modules should not be empty".to_string(),
            ));
        }

        Ok(())
    }

    fn build_audit_details(
        &self,
        request: &NodeAgentRegistrationRequest,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> serde_json::Value {
        json!({
            "cluster_id": request.cluster_id.as_str(),
            "advertise_addr": request.advertise_addr.as_str(),
            "role": request.role.as_str(),
            "running_version": request.running_version.as_str(),
            "state": request.state.as_str(),
            "admin_socket_path": request.admin_socket_path.as_str(),
            "snapshot_version": request.runtime.snapshot_version,
            "runtime_revision": request.runtime.revision,
            "runtime_pid": request.runtime.pid,
            "listener_count": request.runtime.listener_count,
            "active_connections": request.runtime.active_connections,
            "status_reason": request.runtime.error.as_deref(),
            "user_agent": user_agent,
            "remote_addr": remote_addr,
        })
    }

    fn generate_id(&self, prefix: &str) -> String {
        let now = unix_time_ms(SystemTime::now());
        let sequence = NODE_EVENT_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{prefix}_{now}_{sequence}")
    }
}

fn unix_time_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().min(u128::from(u64::MAX)) as u64
}
