use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;
use serde_json::json;

use rginx_control_store::{
    ControlPlaneStore, CreateDeploymentRecord, CreateDeploymentTargetRecord,
    DeploymentProgressSnapshot, NewAuditLogEntry, TaskCompletionRecord,
};
use rginx_control_types::{
    AuthenticatedActor, CreateDeploymentRequest, CreateDeploymentResponse, DeploymentDetail,
    DeploymentStatus, DeploymentSummary, NodeAgentTaskAckResponse, NodeAgentTaskCompleteRequest,
    NodeAgentTaskCompleteResponse, NodeAgentTaskPollResponse, NodeLifecycleState,
};

use crate::{ServiceError, ServiceResult};

static DEPLOYMENT_EVENT_COUNTER: AtomicU64 = AtomicU64::new(1);

const DEPLOYMENT_RESOURCE_TYPE: &str = "deployment";
const DEPLOYMENT_TASK_RESOURCE_TYPE: &str = "deployment_task";
const ACTIVE_CLUSTER_DEPLOYMENT_INDEX: &str = "cp_deployments_one_active_per_cluster_idx";

#[derive(Debug, Clone, Default)]
pub struct DeploymentReconcileReport {
    pub active_deployments: usize,
    pub dispatched_targets: u32,
    pub finalized_deployments: u32,
    pub rollback_deployments_created: u32,
}

#[derive(Debug, Clone)]
pub struct DeploymentService {
    store: ControlPlaneStore,
}

impl DeploymentService {
    pub fn new(store: ControlPlaneStore) -> Self {
        Self { store }
    }

    pub async fn list_deployments(&self) -> ServiceResult<Vec<DeploymentSummary>> {
        self.store
            .deployment_repository()
            .list_deployments()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))
    }

    pub async fn get_deployment_detail(
        &self,
        deployment_id: &str,
    ) -> ServiceResult<DeploymentDetail> {
        self.store
            .deployment_repository()
            .load_deployment_detail(deployment_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .ok_or_else(|| {
                ServiceError::NotFound(format!("deployment `{deployment_id}` was not found"))
            })
    }

    pub async fn create_deployment(
        &self,
        actor: &AuthenticatedActor,
        request: CreateDeploymentRequest,
        idempotency_key: Option<String>,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> ServiceResult<CreateDeploymentResponse> {
        self.validate_create_request(&request)?;
        let idempotency_key = normalize_idempotency_key(idempotency_key)?;
        if let Some(ref key) = idempotency_key
            && let Some(existing_id) = self
                .store
                .deployment_repository()
                .find_id_by_idempotency_key(key)
                .await
                .map_err(|error| ServiceError::Internal(error.to_string()))?
        {
            let deployment = self.get_deployment_detail(&existing_id).await?;
            return Ok(CreateDeploymentResponse { deployment, reused: true });
        }

        if let Some(active) = self
            .store
            .deployment_repository()
            .find_active_cluster_deployment(&request.cluster_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
        {
            return Err(ServiceError::Conflict(format!(
                "deployment lock not acquired for cluster `{}`; active deployment `{}` is still {}",
                request.cluster_id,
                active.deployment_id,
                active.status.as_str()
            )));
        }

        let revision = self
            .store
            .revision_repository()
            .load_revision_detail(&request.revision_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .ok_or_else(|| {
                ServiceError::NotFound(format!("revision `{}` was not found", request.revision_id))
            })?;
        if revision.cluster_id != request.cluster_id {
            return Err(ServiceError::BadRequest(format!(
                "revision `{}` does not belong to cluster `{}`",
                request.revision_id, request.cluster_id
            )));
        }

        let target_nodes = self.resolve_target_nodes(&request).await?;
        let target_count = u32::try_from(target_nodes.len())
            .map_err(|_| ServiceError::Internal("target count should fit into u32".to_string()))?;
        let parallelism = normalize_parallelism(request.parallelism, target_count)?;
        let failure_threshold =
            normalize_failure_threshold(request.failure_threshold, target_count)?;
        let auto_rollback = request.auto_rollback.unwrap_or(false);
        let rollback_revision_id =
            self.select_rollback_revision_id(&request.cluster_id, &request.revision_id).await?;
        let now = Utc::now();
        let deployment_id = self.generate_id("deploy");
        let deployment = self
            .store
            .deployment_repository()
            .create_deployment_with_audit(
                &CreateDeploymentRecord {
                    deployment_id: deployment_id.clone(),
                    cluster_id: request.cluster_id.clone(),
                    revision_id: request.revision_id.clone(),
                    created_by: actor.user.username.clone(),
                    parallelism,
                    failure_threshold,
                    auto_rollback,
                    rollback_of_deployment_id: None,
                    rollback_revision_id: rollback_revision_id.clone(),
                    idempotency_key: idempotency_key.clone(),
                    created_at: now,
                },
                &target_nodes
                    .iter()
                    .enumerate()
                    .map(|(index, node)| CreateDeploymentTargetRecord {
                        target_id: format!("target_{}_{}", deployment_id, index + 1),
                        cluster_id: request.cluster_id.clone(),
                        node_id: node.node_id.clone(),
                        desired_revision_id: request.revision_id.clone(),
                        batch_index: u32::try_from(index)
                            .unwrap_or(u32::MAX)
                            .checked_div(parallelism.max(1))
                            .unwrap_or_default(),
                    })
                    .collect::<Vec<_>>(),
                &NewAuditLogEntry {
                    audit_id: self.generate_id("audit"),
                    request_id: request_id.to_string(),
                    cluster_id: Some(request.cluster_id.clone()),
                    actor_id: actor.user.user_id.clone(),
                    action: "deployment.created".to_string(),
                    resource_type: DEPLOYMENT_RESOURCE_TYPE.to_string(),
                    resource_id: deployment_id.clone(),
                    result: "succeeded".to_string(),
                    details: json!({
                        "revision_id": request.revision_id,
                        "target_node_ids": target_nodes.iter().map(|node| node.node_id.clone()).collect::<Vec<_>>(),
                        "parallelism": parallelism,
                        "failure_threshold": failure_threshold,
                        "auto_rollback": auto_rollback,
                        "rollback_revision_id": rollback_revision_id,
                        "idempotency_key": idempotency_key,
                        "user_agent": user_agent,
                        "remote_addr": remote_addr,
                    }),
                    created_at: now,
                },
            )
            .await
            .map_err(|error| {
                let error_message = error.to_string();
                if let Some(ref key) = idempotency_key
                    && error_message.contains("cp_deployments_idempotency_key_idx")
                {
                    return ServiceError::Conflict(format!(
                        "deployment idempotency key `{key}` already exists"
                    ));
                }
                if error_message.contains(ACTIVE_CLUSTER_DEPLOYMENT_INDEX) {
                    return ServiceError::Conflict(format!(
                        "deployment lock not acquired for cluster `{}`; another active deployment already exists",
                        request.cluster_id
                    ));
                }

                ServiceError::Internal(error_message)
            })?;

        Ok(CreateDeploymentResponse { deployment, reused: false })
    }

    pub async fn pause_deployment(
        &self,
        actor: &AuthenticatedActor,
        deployment_id: &str,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> ServiceResult<DeploymentDetail> {
        let deployment = self.get_deployment_detail(deployment_id).await?;
        match deployment.deployment.status {
            DeploymentStatus::Running => {}
            DeploymentStatus::Paused => {
                return Ok(deployment);
            }
            other => {
                return Err(ServiceError::Conflict(format!(
                    "deployment `{deployment_id}` cannot be paused from `{}`",
                    other.as_str()
                )));
            }
        }

        self.store
            .deployment_repository()
            .set_deployment_paused(deployment_id, true)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        self.record_audit(NewAuditLogEntry {
            audit_id: self.generate_id("audit"),
            request_id: request_id.to_string(),
            cluster_id: Some(deployment.deployment.cluster_id.clone()),
            actor_id: actor.user.user_id.clone(),
            action: "deployment.paused".to_string(),
            resource_type: DEPLOYMENT_RESOURCE_TYPE.to_string(),
            resource_id: deployment_id.to_string(),
            result: "succeeded".to_string(),
            details: json!({
                "user_agent": user_agent,
                "remote_addr": remote_addr,
            }),
            created_at: Utc::now(),
        })
        .await?;

        self.get_deployment_detail(deployment_id).await
    }

    pub async fn resume_deployment(
        &self,
        actor: &AuthenticatedActor,
        deployment_id: &str,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> ServiceResult<DeploymentDetail> {
        let deployment = self.get_deployment_detail(deployment_id).await?;
        match deployment.deployment.status {
            DeploymentStatus::Paused => {}
            DeploymentStatus::Running => {
                return Ok(deployment);
            }
            other => {
                return Err(ServiceError::Conflict(format!(
                    "deployment `{deployment_id}` cannot be resumed from `{}`",
                    other.as_str()
                )));
            }
        }

        self.store
            .deployment_repository()
            .set_deployment_paused(deployment_id, false)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        self.record_audit(NewAuditLogEntry {
            audit_id: self.generate_id("audit"),
            request_id: request_id.to_string(),
            cluster_id: Some(deployment.deployment.cluster_id.clone()),
            actor_id: actor.user.user_id.clone(),
            action: "deployment.resumed".to_string(),
            resource_type: DEPLOYMENT_RESOURCE_TYPE.to_string(),
            resource_id: deployment_id.to_string(),
            result: "succeeded".to_string(),
            details: json!({
                "user_agent": user_agent,
                "remote_addr": remote_addr,
            }),
            created_at: Utc::now(),
        })
        .await?;

        self.get_deployment_detail(deployment_id).await
    }

    pub async fn poll_task(
        &self,
        node_id: &str,
        cluster_id: &str,
    ) -> ServiceResult<NodeAgentTaskPollResponse> {
        validate_node_identity(node_id, cluster_id)?;
        let task = self
            .store
            .deployment_repository()
            .poll_task_for_node(node_id, cluster_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;

        Ok(NodeAgentTaskPollResponse {
            task,
            polled_at_unix_ms: unix_time_ms(SystemTime::now()),
            agent_token: None,
            agent_token_expires_at_unix_ms: None,
        })
    }

    pub async fn ack_task(
        &self,
        task_id: &str,
        node_id: &str,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> ServiceResult<NodeAgentTaskAckResponse> {
        if node_id.trim().is_empty() {
            return Err(ServiceError::BadRequest("node_id should not be empty".to_string()));
        }

        let response = self
            .store
            .deployment_repository()
            .ack_task(task_id, node_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .ok_or_else(|| ServiceError::NotFound(format!("task `{task_id}` was not found")))?;
        self.record_audit(NewAuditLogEntry {
            audit_id: self.generate_id("audit"),
            request_id: request_id.to_string(),
            cluster_id: None,
            actor_id: format!("agent:{node_id}"),
            action: "deployment.task_acknowledged".to_string(),
            resource_type: DEPLOYMENT_TASK_RESOURCE_TYPE.to_string(),
            resource_id: task_id.to_string(),
            result: "succeeded".to_string(),
            details: json!({
                "deployment_id": response.deployment_id,
                "state": response.state.as_str(),
                "acknowledged_at_unix_ms": response.acknowledged_at_unix_ms,
                "user_agent": user_agent,
                "remote_addr": remote_addr,
            }),
            created_at: Utc::now(),
        })
        .await?;

        Ok(response)
    }

    pub async fn complete_task(
        &self,
        task_id: &str,
        node_id: &str,
        request: NodeAgentTaskCompleteRequest,
        idempotency_key: Option<String>,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> ServiceResult<NodeAgentTaskCompleteResponse> {
        if node_id.trim().is_empty() {
            return Err(ServiceError::BadRequest("node_id should not be empty".to_string()));
        }

        let response = self
            .store
            .deployment_repository()
            .complete_task(
                task_id,
                node_id,
                &TaskCompletionRecord {
                    succeeded: request.succeeded,
                    message: request.message.clone(),
                    runtime_revision: request.runtime_revision,
                    completion_idempotency_key: normalize_idempotency_key(idempotency_key)?,
                },
            )
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .ok_or_else(|| ServiceError::NotFound(format!("task `{task_id}` was not found")))?;
        self.record_audit(NewAuditLogEntry {
            audit_id: self.generate_id("audit"),
            request_id: request_id.to_string(),
            cluster_id: None,
            actor_id: format!("agent:{node_id}"),
            action: if request.succeeded {
                "deployment.task_succeeded".to_string()
            } else {
                "deployment.task_failed".to_string()
            },
            resource_type: DEPLOYMENT_TASK_RESOURCE_TYPE.to_string(),
            resource_id: task_id.to_string(),
            result: if request.succeeded { "succeeded" } else { "failed" }.to_string(),
            details: json!({
                "deployment_id": response.deployment_id,
                "state": response.state.as_str(),
                "message": request.message,
                "runtime_revision": request.runtime_revision,
                "completed_at_unix_ms": response.completed_at_unix_ms,
                "user_agent": user_agent,
                "remote_addr": remote_addr,
            }),
            created_at: Utc::now(),
        })
        .await?;

        Ok(response)
    }

    pub async fn reconcile_deployments(&self) -> ServiceResult<DeploymentReconcileReport> {
        let active_deployments = self
            .store
            .deployment_repository()
            .list_active_deployment_ids()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        let mut report = DeploymentReconcileReport {
            active_deployments: active_deployments.len(),
            ..Default::default()
        };

        for deployment_id in active_deployments {
            let Some(progress) = self
                .store
                .deployment_repository()
                .load_progress_snapshot(&deployment_id)
                .await
                .map_err(|error| ServiceError::Internal(error.to_string()))?
            else {
                continue;
            };
            if progress.status == DeploymentStatus::Paused {
                continue;
            }

            if should_block_new_dispatch(&progress) {
                if progress.pending_nodes > 0 {
                    report.dispatched_targets = report.dispatched_targets.saturating_add(
                        self.store
                            .deployment_repository()
                            .cancel_pending_targets(
                                &progress.deployment_id,
                                "failure threshold reached; pending targets cancelled",
                            )
                            .await
                            .map_err(|error| ServiceError::Internal(error.to_string()))?,
                    );
                }
                if progress.in_flight_nodes > 0 {
                    self.store
                        .deployment_repository()
                        .set_status_reason(
                            &progress.deployment_id,
                            Some("failure threshold reached; waiting for in-flight targets"),
                        )
                        .await
                        .map_err(|error| ServiceError::Internal(error.to_string()))?;
                    continue;
                }

                if self
                    .finalize_failed_deployment(
                        &progress,
                        format!("failure threshold {} reached", progress.failure_threshold),
                    )
                    .await?
                {
                    report.finalized_deployments = report.finalized_deployments.saturating_add(1);
                    report.rollback_deployments_created = report
                        .rollback_deployments_created
                        .saturating_add(self.maybe_create_auto_rollback(&progress).await?);
                }
                continue;
            }

            if should_finalize_now(&progress) {
                if progress.failed_nodes == 0 {
                    if self.finalize_successful_deployment(&progress).await? {
                        report.finalized_deployments =
                            report.finalized_deployments.saturating_add(1);
                    }
                } else if self
                    .finalize_failed_deployment(
                        &progress,
                        format!("{} target(s) failed", progress.failed_nodes),
                    )
                    .await?
                {
                    report.finalized_deployments = report.finalized_deployments.saturating_add(1);
                    report.rollback_deployments_created = report
                        .rollback_deployments_created
                        .saturating_add(self.maybe_create_auto_rollback(&progress).await?);
                }
                continue;
            }

            let capacity = dispatch_capacity(progress.parallelism, progress.in_flight_nodes);
            if capacity > 0 {
                report.dispatched_targets = report.dispatched_targets.saturating_add(
                    self.store
                        .deployment_repository()
                        .dispatch_next_targets(&progress.deployment_id, capacity)
                        .await
                        .map_err(|error| ServiceError::Internal(error.to_string()))?,
                );
            }
        }

        Ok(report)
    }

    async fn resolve_target_nodes(
        &self,
        request: &CreateDeploymentRequest,
    ) -> ServiceResult<Vec<rginx_control_types::NodeSummary>> {
        let eligible_nodes = self
            .store
            .node_repository()
            .list_nodes()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .into_iter()
            .filter(|node| node.cluster_id == request.cluster_id)
            .filter(|node| {
                matches!(node.state, NodeLifecycleState::Online | NodeLifecycleState::Draining)
            })
            .collect::<Vec<_>>();

        if eligible_nodes.is_empty() {
            return Err(ServiceError::BadRequest(format!(
                "cluster `{}` has no online or draining nodes eligible for deployment",
                request.cluster_id
            )));
        }

        if let Some(target_node_ids) = request.target_node_ids.as_ref() {
            if target_node_ids.is_empty() {
                return Err(ServiceError::BadRequest(
                    "target_node_ids should not be an empty list".to_string(),
                ));
            }

            let mut requested = BTreeSet::new();
            for node_id in target_node_ids {
                if !requested.insert(node_id.clone()) {
                    return Err(ServiceError::BadRequest(format!(
                        "target_node_ids contains duplicate node `{node_id}`"
                    )));
                }
            }

            let by_id = eligible_nodes
                .into_iter()
                .map(|node| (node.node_id.clone(), node))
                .collect::<BTreeMap<_, _>>();
            let mut selected = Vec::with_capacity(target_node_ids.len());
            for node_id in target_node_ids {
                let Some(node) = by_id.get(node_id) else {
                    return Err(ServiceError::BadRequest(format!(
                        "node `{node_id}` is not online/draining in cluster `{}`",
                        request.cluster_id
                    )));
                };
                selected.push(node.clone());
            }

            Ok(selected)
        } else {
            Ok(eligible_nodes)
        }
    }

    async fn select_rollback_revision_id(
        &self,
        cluster_id: &str,
        revision_id: &str,
    ) -> ServiceResult<Option<String>> {
        let revision = self
            .store
            .revision_repository()
            .list_revisions()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .into_iter()
            .find(|entry| entry.cluster_id == cluster_id && entry.revision_id != revision_id)
            .map(|entry| entry.revision_id);
        Ok(revision)
    }

    async fn finalize_successful_deployment(
        &self,
        progress: &DeploymentProgressSnapshot,
    ) -> ServiceResult<bool> {
        let Some(summary) = self
            .store
            .deployment_repository()
            .mark_deployment_succeeded(&progress.deployment_id, None)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
        else {
            return Ok(false);
        };

        self.record_audit(NewAuditLogEntry {
            audit_id: self.generate_id("audit"),
            request_id: self.generate_id("req_worker"),
            cluster_id: Some(progress.cluster_id.clone()),
            actor_id: "system:worker".to_string(),
            action: "deployment.succeeded".to_string(),
            resource_type: DEPLOYMENT_RESOURCE_TYPE.to_string(),
            resource_id: summary.deployment_id.clone(),
            result: "succeeded".to_string(),
            details: json!({
                "healthy_nodes": summary.healthy_nodes,
                "target_nodes": summary.target_nodes,
            }),
            created_at: Utc::now(),
        })
        .await?;

        if let Some(source_deployment_id) = summary.rollback_of_deployment_id.clone() {
            self.store
                .deployment_repository()
                .mark_deployment_rolled_back(
                    &source_deployment_id,
                    &format!("rollback deployment `{}` succeeded", summary.deployment_id),
                )
                .await
                .map_err(|error| ServiceError::Internal(error.to_string()))?;
            self.record_audit(NewAuditLogEntry {
                audit_id: self.generate_id("audit"),
                request_id: self.generate_id("req_worker"),
                cluster_id: Some(progress.cluster_id.clone()),
                actor_id: "system:worker".to_string(),
                action: "deployment.rolled_back".to_string(),
                resource_type: DEPLOYMENT_RESOURCE_TYPE.to_string(),
                resource_id: source_deployment_id,
                result: "succeeded".to_string(),
                details: json!({
                    "rollback_deployment_id": summary.deployment_id,
                }),
                created_at: Utc::now(),
            })
            .await?;
        }

        Ok(true)
    }

    async fn finalize_failed_deployment(
        &self,
        progress: &DeploymentProgressSnapshot,
        reason: String,
    ) -> ServiceResult<bool> {
        let Some(summary) = self
            .store
            .deployment_repository()
            .mark_deployment_failed(&progress.deployment_id, &reason)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
        else {
            return Ok(false);
        };

        self.record_audit(NewAuditLogEntry {
            audit_id: self.generate_id("audit"),
            request_id: self.generate_id("req_worker"),
            cluster_id: Some(progress.cluster_id.clone()),
            actor_id: "system:worker".to_string(),
            action: "deployment.failed".to_string(),
            resource_type: DEPLOYMENT_RESOURCE_TYPE.to_string(),
            resource_id: summary.deployment_id,
            result: "failed".to_string(),
            details: json!({
                "reason": reason,
                "healthy_nodes": summary.healthy_nodes,
                "failed_nodes": summary.failed_nodes,
                "target_nodes": summary.target_nodes,
            }),
            created_at: Utc::now(),
        })
        .await?;

        Ok(true)
    }

    async fn maybe_create_auto_rollback(
        &self,
        progress: &DeploymentProgressSnapshot,
    ) -> ServiceResult<u32> {
        if !progress.auto_rollback
            || progress.rollback_of_deployment_id.is_some()
            || progress.healthy_nodes == 0
        {
            return Ok(0);
        }
        let Some(rollback_revision_id) = progress.rollback_revision_id.clone() else {
            return Ok(0);
        };
        if self
            .store
            .deployment_repository()
            .find_rollback_child(&progress.deployment_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .is_some()
        {
            return Ok(0);
        }

        let target_node_ids = self
            .store
            .deployment_repository()
            .list_succeeded_target_node_ids(&progress.deployment_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        if target_node_ids.is_empty() {
            return Ok(0);
        }

        let idempotency_key = format!("auto-rollback:{}", progress.deployment_id);
        if let Some(existing_id) = self
            .store
            .deployment_repository()
            .find_id_by_idempotency_key(&idempotency_key)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
        {
            if self.get_deployment_detail(&existing_id).await.is_ok() {
                return Ok(0);
            }
        }

        let now = Utc::now();
        let rollback_deployment_id = self.generate_id("deploy");
        let create_result = self
            .store
            .deployment_repository()
            .create_deployment_with_audit(
                &CreateDeploymentRecord {
                    deployment_id: rollback_deployment_id.clone(),
                    cluster_id: progress.cluster_id.clone(),
                    revision_id: rollback_revision_id.clone(),
                    created_by: "system:worker".to_string(),
                    parallelism: progress.parallelism.max(1),
                    failure_threshold: normalize_failure_threshold(
                        Some(progress.failure_threshold),
                        progress.healthy_nodes,
                    )?,
                    auto_rollback: false,
                    rollback_of_deployment_id: Some(progress.deployment_id.clone()),
                    rollback_revision_id: None,
                    idempotency_key: Some(idempotency_key),
                    created_at: now,
                },
                &target_node_ids
                    .iter()
                    .enumerate()
                    .map(|(index, node_id)| CreateDeploymentTargetRecord {
                        target_id: format!("target_{}_{}", rollback_deployment_id, index + 1),
                        cluster_id: progress.cluster_id.clone(),
                        node_id: node_id.clone(),
                        desired_revision_id: rollback_revision_id.clone(),
                        batch_index: u32::try_from(index)
                            .unwrap_or(u32::MAX)
                            .checked_div(progress.parallelism.max(1))
                            .unwrap_or_default(),
                    })
                    .collect::<Vec<_>>(),
                &NewAuditLogEntry {
                    audit_id: self.generate_id("audit"),
                    request_id: self.generate_id("req_worker"),
                    cluster_id: Some(progress.cluster_id.clone()),
                    actor_id: "system:worker".to_string(),
                    action: "deployment.rollback_created".to_string(),
                    resource_type: DEPLOYMENT_RESOURCE_TYPE.to_string(),
                    resource_id: rollback_deployment_id,
                    result: "succeeded".to_string(),
                    details: json!({
                        "rollback_of_deployment_id": progress.deployment_id,
                        "rollback_revision_id": rollback_revision_id,
                        "target_node_ids": target_node_ids,
                    }),
                    created_at: now,
                },
            )
            .await;
        if let Err(error) = create_result {
            let error_message = error.to_string();
            if error_message.contains(ACTIVE_CLUSTER_DEPLOYMENT_INDEX) {
                tracing::warn!(
                    cluster_id = %progress.cluster_id,
                    deployment_id = %progress.deployment_id,
                    "skipping auto rollback because another active deployment already exists"
                );
                return Ok(0);
            }
            return Err(ServiceError::Internal(error_message));
        }

        Ok(1)
    }

    async fn record_audit(&self, entry: NewAuditLogEntry) -> ServiceResult<()> {
        self.store
            .audit_repository()
            .insert_entry(&entry)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))
    }

    fn validate_create_request(&self, request: &CreateDeploymentRequest) -> ServiceResult<()> {
        if request.cluster_id.trim().is_empty() {
            return Err(ServiceError::BadRequest("cluster_id should not be empty".to_string()));
        }
        if request.revision_id.trim().is_empty() {
            return Err(ServiceError::BadRequest("revision_id should not be empty".to_string()));
        }
        Ok(())
    }

    fn generate_id(&self, prefix: &str) -> String {
        let now = unix_time_ms(SystemTime::now());
        let sequence = DEPLOYMENT_EVENT_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{prefix}_{now}_{sequence}")
    }
}

fn validate_node_identity(node_id: &str, cluster_id: &str) -> ServiceResult<()> {
    if node_id.trim().is_empty() {
        return Err(ServiceError::BadRequest("node_id should not be empty".to_string()));
    }
    if cluster_id.trim().is_empty() {
        return Err(ServiceError::BadRequest("cluster_id should not be empty".to_string()));
    }
    Ok(())
}

fn normalize_parallelism(value: Option<u32>, target_count: u32) -> ServiceResult<u32> {
    match value.unwrap_or(1) {
        0 => Err(ServiceError::BadRequest("parallelism should be greater than zero".to_string())),
        value => Ok(value.min(target_count.max(1))),
    }
}

fn normalize_failure_threshold(value: Option<u32>, target_count: u32) -> ServiceResult<u32> {
    match value.unwrap_or(1) {
        0 => Err(ServiceError::BadRequest(
            "failure_threshold should be greater than zero".to_string(),
        )),
        value => Ok(value.min(target_count.max(1))),
    }
}

fn normalize_idempotency_key(value: Option<String>) -> ServiceResult<Option<String>> {
    match value {
        Some(value) if value.trim().is_empty() => {
            Err(ServiceError::BadRequest("Idempotency-Key should not be empty".to_string()))
        }
        Some(value) => Ok(Some(value.trim().to_string())),
        None => Ok(None),
    }
}

fn dispatch_capacity(parallelism: u32, in_flight_nodes: u32) -> u32 {
    parallelism.saturating_sub(in_flight_nodes)
}

fn should_block_new_dispatch(progress: &DeploymentProgressSnapshot) -> bool {
    progress.failed_nodes > 0 && progress.failed_nodes >= progress.failure_threshold
}

fn should_finalize_now(progress: &DeploymentProgressSnapshot) -> bool {
    progress.pending_nodes == 0 && progress.in_flight_nodes == 0
}

fn unix_time_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use rginx_control_store::DeploymentProgressSnapshot;
    use rginx_control_types::DeploymentStatus;

    use super::{
        dispatch_capacity, normalize_failure_threshold, normalize_parallelism,
        should_block_new_dispatch, should_finalize_now,
    };

    #[test]
    fn parallelism_is_bounded_by_target_count() {
        assert_eq!(normalize_parallelism(Some(4), 2).expect("parallelism should normalize"), 2);
    }

    #[test]
    fn failure_threshold_is_bounded_by_target_count() {
        assert_eq!(
            normalize_failure_threshold(Some(5), 3).expect("failure_threshold should normalize"),
            3
        );
    }

    #[test]
    fn dispatch_capacity_never_underflows() {
        assert_eq!(dispatch_capacity(2, 3), 0);
        assert_eq!(dispatch_capacity(4, 1), 3);
    }

    #[test]
    fn progress_blocks_new_dispatch_after_threshold() {
        let progress = DeploymentProgressSnapshot {
            deployment_id: "deploy_1".to_string(),
            cluster_id: "cluster-mainland".to_string(),
            status: DeploymentStatus::Running,
            parallelism: 2,
            failure_threshold: 1,
            auto_rollback: true,
            rollback_of_deployment_id: None,
            rollback_revision_id: Some("rev_previous".to_string()),
            total_nodes: 3,
            pending_nodes: 1,
            in_flight_nodes: 1,
            healthy_nodes: 1,
            failed_nodes: 1,
        };

        assert!(should_block_new_dispatch(&progress));
        assert!(!should_finalize_now(&progress));
    }

    #[test]
    fn progress_finalizes_when_no_pending_or_in_flight_targets_remain() {
        let progress = DeploymentProgressSnapshot {
            deployment_id: "deploy_1".to_string(),
            cluster_id: "cluster-mainland".to_string(),
            status: DeploymentStatus::Running,
            parallelism: 2,
            failure_threshold: 2,
            auto_rollback: false,
            rollback_of_deployment_id: None,
            rollback_revision_id: None,
            total_nodes: 3,
            pending_nodes: 0,
            in_flight_nodes: 0,
            healthy_nodes: 2,
            failed_nodes: 1,
        };

        assert!(should_finalize_now(&progress));
    }
}
