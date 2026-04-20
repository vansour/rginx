use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use serde_json::json;

use rginx_control_store::{
    ActiveDnsDeploymentTargetObservation, ControlPlaneStore, CreateDnsDeploymentRecord,
    CreateDnsDeploymentTargetRecord, DnsDeploymentProgressSnapshot, NewAuditLogEntry,
};
use rginx_control_types::{
    AuthenticatedActor, CreateDnsDeploymentRequest, CreateDnsDeploymentResponse,
    DnsDeploymentDetail, DnsDeploymentStatus, DnsDeploymentSummary, NodeLifecycleState,
};

use crate::{ServiceError, ServiceResult};

static DNS_DEPLOYMENT_EVENT_COUNTER: AtomicU64 = AtomicU64::new(1);

const DNS_DEPLOYMENT_RESOURCE_TYPE: &str = "dns_deployment";
const ACTIVE_CLUSTER_DEPLOYMENT_INDEX: &str = "cp_dns_deployments_one_active_per_cluster_idx";
const DNS_TARGET_CONFIRM_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, Default)]
pub struct DnsDeploymentReconcileReport {
    pub active_deployments: usize,
    pub assigned_targets: u32,
    pub finalized_deployments: u32,
    pub rollback_deployments_created: u32,
}

#[derive(Debug, Clone)]
pub struct DnsDeploymentService {
    store: ControlPlaneStore,
}

#[derive(Debug, Clone)]
struct CreateDnsDeploymentSpec {
    cluster_id: String,
    revision_id: String,
    target_node_ids: Vec<String>,
    parallelism: Option<u32>,
    failure_threshold: Option<u32>,
    auto_rollback: bool,
    promotes_cluster_runtime: bool,
    idempotency_key: Option<String>,
    rollback_of_deployment_id: Option<String>,
    created_by: String,
    actor_id: String,
    request_id: String,
    user_agent: Option<String>,
    remote_addr: Option<String>,
    action: String,
    allow_conflicting_overrides: bool,
    allow_offline_targets: bool,
}

impl DnsDeploymentService {
    pub fn new(store: ControlPlaneStore) -> Self {
        Self { store }
    }

    pub async fn list_deployments(&self) -> ServiceResult<Vec<DnsDeploymentSummary>> {
        self.store
            .dns_deployment_repository()
            .list_deployments()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))
    }

    pub async fn get_deployment_detail(
        &self,
        deployment_id: &str,
    ) -> ServiceResult<DnsDeploymentDetail> {
        self.store
            .dns_deployment_repository()
            .load_deployment_detail(deployment_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .ok_or_else(|| {
                ServiceError::NotFound(format!("dns deployment `{deployment_id}` was not found"))
            })
    }

    pub async fn create_deployment(
        &self,
        actor: &AuthenticatedActor,
        request: CreateDnsDeploymentRequest,
        idempotency_key: Option<String>,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> ServiceResult<CreateDnsDeploymentResponse> {
        self.validate_create_request(&request.cluster_id, &request.revision_id)?;
        let idempotency_key = normalize_idempotency_key(idempotency_key)?;
        if let Some(ref key) = idempotency_key
            && let Some(existing_id) = self
                .store
                .dns_deployment_repository()
                .find_id_by_idempotency_key(key)
                .await
                .map_err(|error| ServiceError::Internal(error.to_string()))?
        {
            let deployment = self.get_deployment_detail(&existing_id).await?;
            return Ok(CreateDnsDeploymentResponse { deployment, reused: true });
        }

        let revision = self
            .store
            .dns_repository()
            .load_revision_detail(&request.revision_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .ok_or_else(|| {
                ServiceError::NotFound(format!(
                    "dns revision `{}` was not found",
                    request.revision_id
                ))
            })?;
        if revision.cluster_id != request.cluster_id {
            return Err(ServiceError::BadRequest(format!(
                "dns revision `{}` does not belong to cluster `{}`",
                request.revision_id, request.cluster_id
            )));
        }

        let (target_nodes, eligible_online_count) = self
            .resolve_target_nodes(&request.cluster_id, request.target_node_ids.as_deref(), false)
            .await?;
        let promotes_cluster_runtime =
            target_nodes.len() == eligible_online_count && eligible_online_count > 0;

        self.create_deployment_with_spec(CreateDnsDeploymentSpec {
            cluster_id: request.cluster_id.clone(),
            revision_id: request.revision_id.clone(),
            target_node_ids: target_nodes.iter().map(|node| node.node_id.clone()).collect(),
            parallelism: request.parallelism,
            failure_threshold: request.failure_threshold,
            auto_rollback: request.auto_rollback.unwrap_or(false),
            promotes_cluster_runtime,
            idempotency_key,
            rollback_of_deployment_id: None,
            created_by: actor.user.username.clone(),
            actor_id: actor.user.user_id.clone(),
            request_id: request_id.to_string(),
            user_agent,
            remote_addr,
            action: "dns.deployment.created".to_string(),
            allow_conflicting_overrides: false,
            allow_offline_targets: false,
        })
        .await
    }

    pub async fn pause_deployment(
        &self,
        actor: &AuthenticatedActor,
        deployment_id: &str,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> ServiceResult<DnsDeploymentDetail> {
        let deployment = self.get_deployment_detail(deployment_id).await?;
        match deployment.deployment.status {
            DnsDeploymentStatus::Running => {}
            DnsDeploymentStatus::Paused => return Ok(deployment),
            other => {
                return Err(ServiceError::Conflict(format!(
                    "dns deployment `{deployment_id}` cannot be paused from `{}`",
                    other.as_str()
                )));
            }
        }

        self.store
            .dns_deployment_repository()
            .set_deployment_paused(deployment_id, true)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        self.record_audit(NewAuditLogEntry {
            audit_id: self.generate_id("audit"),
            request_id: request_id.to_string(),
            cluster_id: Some(deployment.deployment.cluster_id.clone()),
            actor_id: actor.user.user_id.clone(),
            action: "dns.deployment.paused".to_string(),
            resource_type: DNS_DEPLOYMENT_RESOURCE_TYPE.to_string(),
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
    ) -> ServiceResult<DnsDeploymentDetail> {
        let deployment = self.get_deployment_detail(deployment_id).await?;
        match deployment.deployment.status {
            DnsDeploymentStatus::Paused => {}
            DnsDeploymentStatus::Running => return Ok(deployment),
            other => {
                return Err(ServiceError::Conflict(format!(
                    "dns deployment `{deployment_id}` cannot be resumed from `{}`",
                    other.as_str()
                )));
            }
        }

        self.store
            .dns_deployment_repository()
            .set_deployment_paused(deployment_id, false)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        self.record_audit(NewAuditLogEntry {
            audit_id: self.generate_id("audit"),
            request_id: request_id.to_string(),
            cluster_id: Some(deployment.deployment.cluster_id.clone()),
            actor_id: actor.user.user_id.clone(),
            action: "dns.deployment.resumed".to_string(),
            resource_type: DNS_DEPLOYMENT_RESOURCE_TYPE.to_string(),
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

    pub async fn rollback_deployment(
        &self,
        actor: &AuthenticatedActor,
        deployment_id: &str,
        idempotency_key: Option<String>,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> ServiceResult<CreateDnsDeploymentResponse> {
        let source = self.get_deployment_detail(deployment_id).await?;
        if source.deployment.rollback_of_deployment_id.is_some() {
            return Err(ServiceError::Conflict(format!(
                "dns deployment `{deployment_id}` is already a rollback deployment"
            )));
        }
        match source.deployment.status {
            DnsDeploymentStatus::Succeeded | DnsDeploymentStatus::Failed => {}
            other => {
                return Err(ServiceError::Conflict(format!(
                    "dns deployment `{deployment_id}` cannot be rolled back from `{}`",
                    other.as_str()
                )));
            }
        }
        let Some(rollback_revision_id) = source.deployment.rollback_revision_id.clone() else {
            return Err(ServiceError::Conflict(format!(
                "dns deployment `{deployment_id}` does not have a stable rollback revision"
            )));
        };
        if let Some(existing) = self
            .store
            .dns_deployment_repository()
            .find_rollback_child(deployment_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
        {
            let deployment = self.get_deployment_detail(&existing.deployment_id).await?;
            return Ok(CreateDnsDeploymentResponse { deployment, reused: true });
        }

        let target_node_ids = source
            .targets
            .iter()
            .filter(|target| {
                target.state == rginx_control_types::DnsDeploymentTargetState::Succeeded
            })
            .map(|target| target.node_id.clone())
            .collect::<Vec<_>>();
        if target_node_ids.is_empty() {
            return Err(ServiceError::Conflict(format!(
                "dns deployment `{deployment_id}` has no succeeded targets to roll back"
            )));
        }

        self.create_deployment_with_spec(CreateDnsDeploymentSpec {
            cluster_id: source.deployment.cluster_id.clone(),
            revision_id: rollback_revision_id,
            target_node_ids,
            parallelism: Some(source.deployment.parallelism),
            failure_threshold: Some(source.deployment.failure_threshold),
            auto_rollback: false,
            promotes_cluster_runtime: source.deployment.status == DnsDeploymentStatus::Succeeded
                && source.deployment.promotes_cluster_runtime,
            idempotency_key: normalize_idempotency_key(idempotency_key)?
                .or_else(|| Some(format!("dns-rollback:{deployment_id}"))),
            rollback_of_deployment_id: Some(source.deployment.deployment_id.clone()),
            created_by: actor.user.username.clone(),
            actor_id: actor.user.user_id.clone(),
            request_id: request_id.to_string(),
            user_agent,
            remote_addr,
            action: "dns.deployment.rollback_created".to_string(),
            allow_conflicting_overrides: true,
            allow_offline_targets: true,
        })
        .await
    }

    pub async fn reconcile_deployments(&self) -> ServiceResult<DnsDeploymentReconcileReport> {
        let active_deployments = self
            .store
            .dns_deployment_repository()
            .list_active_deployment_ids()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        let mut report = DnsDeploymentReconcileReport {
            active_deployments: active_deployments.len(),
            ..Default::default()
        };

        for deployment_id in active_deployments {
            self.reconcile_active_targets(&deployment_id).await?;
            let Some(progress) = self
                .store
                .dns_deployment_repository()
                .load_progress_snapshot(&deployment_id)
                .await
                .map_err(|error| ServiceError::Internal(error.to_string()))?
            else {
                continue;
            };
            if progress.status == DnsDeploymentStatus::Paused {
                continue;
            }

            if should_block_new_dispatch(&progress) {
                if progress.pending_nodes > 0 {
                    report.assigned_targets = report.assigned_targets.saturating_add(
                        self.store
                            .dns_deployment_repository()
                            .cancel_pending_targets(
                                &progress.deployment_id,
                                "failure threshold reached; pending targets cancelled",
                            )
                            .await
                            .map_err(|error| ServiceError::Internal(error.to_string()))?,
                    );
                }
                if progress.active_nodes > 0 {
                    self.store
                        .dns_deployment_repository()
                        .set_status_reason(
                            &progress.deployment_id,
                            Some("failure threshold reached; waiting for active targets"),
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

            let capacity = dispatch_capacity(progress.parallelism, progress.active_nodes);
            if capacity > 0 {
                report.assigned_targets = report.assigned_targets.saturating_add(
                    self.store
                        .dns_deployment_repository()
                        .activate_next_targets(&progress.deployment_id, capacity)
                        .await
                        .map_err(|error| ServiceError::Internal(error.to_string()))?,
                );
            }
        }

        Ok(report)
    }

    async fn create_deployment_with_spec(
        &self,
        spec: CreateDnsDeploymentSpec,
    ) -> ServiceResult<CreateDnsDeploymentResponse> {
        if let Some(ref key) = spec.idempotency_key
            && let Some(existing_id) = self
                .store
                .dns_deployment_repository()
                .find_id_by_idempotency_key(key)
                .await
                .map_err(|error| ServiceError::Internal(error.to_string()))?
        {
            let deployment = self.get_deployment_detail(&existing_id).await?;
            return Ok(CreateDnsDeploymentResponse { deployment, reused: true });
        }
        if let Some(active) = self
            .store
            .dns_deployment_repository()
            .find_active_cluster_deployment(&spec.cluster_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
        {
            return Err(ServiceError::Conflict(format!(
                "dns deployment lock not acquired for cluster `{}`; active deployment `{}` is still {}",
                spec.cluster_id,
                active.deployment_id,
                active.status.as_str()
            )));
        }

        let revision = self
            .store
            .dns_repository()
            .load_revision_detail(&spec.revision_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .ok_or_else(|| {
                ServiceError::NotFound(format!("dns revision `{}` was not found", spec.revision_id))
            })?;
        if revision.cluster_id != spec.cluster_id {
            return Err(ServiceError::BadRequest(format!(
                "dns revision `{}` does not belong to cluster `{}`",
                spec.revision_id, spec.cluster_id
            )));
        }

        let overrides = self
            .store
            .dns_deployment_repository()
            .list_cluster_node_overrides(&spec.cluster_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        if !spec.allow_conflicting_overrides {
            let conflicting =
                overrides.iter().find(|item| item.published_revision_id != spec.revision_id);
            if let Some(conflicting) = conflicting {
                return Err(ServiceError::Conflict(format!(
                    "cluster `{}` already has dns canary node `{}` pinned to revision `{}`; roll it back or continue deploying the same revision first",
                    spec.cluster_id, conflicting.node_id, conflicting.published_revision_id
                )));
            }
        }

        let (target_nodes, _) = self
            .resolve_target_nodes(
                &spec.cluster_id,
                Some(&spec.target_node_ids),
                spec.allow_offline_targets,
            )
            .await?;
        let target_count = u32::try_from(target_nodes.len())
            .map_err(|_| ServiceError::Internal("target count should fit into u32".to_string()))?;
        let parallelism = normalize_parallelism(spec.parallelism, target_count)?;
        let failure_threshold = normalize_failure_threshold(spec.failure_threshold, target_count)?;
        let rollback_revision_id = self
            .store
            .dns_deployment_repository()
            .load_runtime_revision_for_cluster(&spec.cluster_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .map(|revision| revision.revision_id);
        let now = Utc::now();
        let deployment_id = self.generate_id("dns_deploy");
        let target_ids = target_nodes.iter().map(|node| node.node_id.clone()).collect::<Vec<_>>();
        let deployment = self
            .store
            .dns_deployment_repository()
            .create_deployment_with_audit(
                &CreateDnsDeploymentRecord {
                    deployment_id: deployment_id.clone(),
                    cluster_id: spec.cluster_id.clone(),
                    revision_id: spec.revision_id.clone(),
                    created_by: spec.created_by.clone(),
                    parallelism,
                    failure_threshold,
                    auto_rollback: spec.auto_rollback,
                    promotes_cluster_runtime: spec.promotes_cluster_runtime,
                    rollback_of_deployment_id: spec.rollback_of_deployment_id.clone(),
                    rollback_revision_id: rollback_revision_id.clone(),
                    idempotency_key: spec.idempotency_key.clone(),
                    created_at: now,
                },
                &target_nodes
                    .iter()
                    .enumerate()
                    .map(|(index, node)| CreateDnsDeploymentTargetRecord {
                        target_id: format!("target_{}_{}", deployment_id, index + 1),
                        cluster_id: spec.cluster_id.clone(),
                        node_id: node.node_id.clone(),
                        desired_revision_id: spec.revision_id.clone(),
                        batch_index: u32::try_from(index)
                            .unwrap_or(u32::MAX)
                            .checked_div(parallelism.max(1))
                            .unwrap_or_default(),
                    })
                    .collect::<Vec<_>>(),
                &NewAuditLogEntry {
                    audit_id: self.generate_id("audit"),
                    request_id: spec.request_id.clone(),
                    cluster_id: Some(spec.cluster_id.clone()),
                    actor_id: spec.actor_id.clone(),
                    action: spec.action,
                    resource_type: DNS_DEPLOYMENT_RESOURCE_TYPE.to_string(),
                    resource_id: deployment_id.clone(),
                    result: "succeeded".to_string(),
                    details: json!({
                        "revision_id": spec.revision_id,
                        "target_node_ids": target_ids,
                        "parallelism": parallelism,
                        "failure_threshold": failure_threshold,
                        "auto_rollback": spec.auto_rollback,
                        "promotes_cluster_runtime": spec.promotes_cluster_runtime,
                        "rollback_of_deployment_id": spec.rollback_of_deployment_id,
                        "rollback_revision_id": rollback_revision_id,
                        "idempotency_key": spec.idempotency_key,
                        "user_agent": spec.user_agent,
                        "remote_addr": spec.remote_addr,
                    }),
                    created_at: now,
                },
            )
            .await
            .map_err(|error| {
                let error_message = error.to_string();
                if let Some(ref key) = spec.idempotency_key
                    && error_message.contains("cp_dns_deployments_idempotency_key_idx")
                {
                    return ServiceError::Conflict(format!(
                        "dns deployment idempotency key `{key}` already exists"
                    ));
                }
                if error_message.contains(ACTIVE_CLUSTER_DEPLOYMENT_INDEX) {
                    return ServiceError::Conflict(format!(
                        "dns deployment lock not acquired for cluster `{}`; another active deployment already exists",
                        spec.cluster_id
                    ));
                }
                ServiceError::Internal(error_message)
            })?;

        Ok(CreateDnsDeploymentResponse { deployment, reused: false })
    }

    async fn reconcile_active_targets(&self, deployment_id: &str) -> ServiceResult<()> {
        let observations = self
            .store
            .dns_deployment_repository()
            .list_active_target_observations(deployment_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        let now = Utc::now();

        for observation in observations {
            if is_target_confirmed(&observation) {
                self.store
                    .dns_deployment_repository()
                    .mark_target_succeeded(&observation.target_id)
                    .await
                    .map_err(|error| ServiceError::Internal(error.to_string()))?;
                continue;
            }
            let Some(assigned_at) = observation.assigned_at else {
                continue;
            };
            let elapsed = now.signed_duration_since(assigned_at);
            if elapsed.to_std().unwrap_or_default() < DNS_TARGET_CONFIRM_TIMEOUT {
                continue;
            }

            let reason = match observation.observed_revision_id.as_deref() {
                Some(observed_revision_id) => format!(
                    "node `{}` did not converge to revision `{}` within {}s (observed `{}`, state `{}`)",
                    observation.node_id,
                    observation.desired_revision_id,
                    DNS_TARGET_CONFIRM_TIMEOUT.as_secs(),
                    observed_revision_id,
                    observation.node_state.as_str()
                ),
                None => format!(
                    "node `{}` did not report dns revision `{}` within {}s (state `{}`)",
                    observation.node_id,
                    observation.desired_revision_id,
                    DNS_TARGET_CONFIRM_TIMEOUT.as_secs(),
                    observation.node_state.as_str()
                ),
            };
            self.store
                .dns_deployment_repository()
                .mark_target_failed(&observation.target_id, &reason)
                .await
                .map_err(|error| ServiceError::Internal(error.to_string()))?;
        }

        Ok(())
    }

    async fn finalize_successful_deployment(
        &self,
        progress: &DnsDeploymentProgressSnapshot,
    ) -> ServiceResult<bool> {
        let Some(summary) = self
            .store
            .dns_deployment_repository()
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
            action: "dns.deployment.succeeded".to_string(),
            resource_type: DNS_DEPLOYMENT_RESOURCE_TYPE.to_string(),
            resource_id: summary.deployment_id.clone(),
            result: "succeeded".to_string(),
            details: json!({
                "healthy_nodes": summary.healthy_nodes,
                "target_nodes": summary.target_nodes,
                "promotes_cluster_runtime": summary.promotes_cluster_runtime,
            }),
            created_at: Utc::now(),
        })
        .await?;

        if let Some(source_deployment_id) = summary.rollback_of_deployment_id.clone() {
            self.store
                .dns_deployment_repository()
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
                action: "dns.deployment.rolled_back".to_string(),
                resource_type: DNS_DEPLOYMENT_RESOURCE_TYPE.to_string(),
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
        progress: &DnsDeploymentProgressSnapshot,
        reason: String,
    ) -> ServiceResult<bool> {
        let Some(summary) = self
            .store
            .dns_deployment_repository()
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
            action: "dns.deployment.failed".to_string(),
            resource_type: DNS_DEPLOYMENT_RESOURCE_TYPE.to_string(),
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
        progress: &DnsDeploymentProgressSnapshot,
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
            .dns_deployment_repository()
            .find_rollback_child(&progress.deployment_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .is_some()
        {
            return Ok(0);
        }

        let target_node_ids = self
            .store
            .dns_deployment_repository()
            .list_succeeded_target_node_ids(&progress.deployment_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        if target_node_ids.is_empty() {
            return Ok(0);
        }

        let create_result = self
            .create_deployment_with_spec(CreateDnsDeploymentSpec {
                cluster_id: progress.cluster_id.clone(),
                revision_id: rollback_revision_id,
                target_node_ids,
                parallelism: Some(progress.parallelism),
                failure_threshold: Some(progress.failure_threshold),
                auto_rollback: false,
                promotes_cluster_runtime: false,
                idempotency_key: Some(format!("auto-dns-rollback:{}", progress.deployment_id)),
                rollback_of_deployment_id: Some(progress.deployment_id.clone()),
                created_by: "system:worker".to_string(),
                actor_id: "system:worker".to_string(),
                request_id: self.generate_id("req_worker"),
                user_agent: None,
                remote_addr: None,
                action: "dns.deployment.rollback_created".to_string(),
                allow_conflicting_overrides: true,
                allow_offline_targets: true,
            })
            .await;

        match create_result {
            Ok(_) => Ok(1),
            Err(ServiceError::Conflict(message))
                if message.contains("another active deployment already exists")
                    || message.contains("lock not acquired") =>
            {
                tracing::warn!(
                    cluster_id = %progress.cluster_id,
                    deployment_id = %progress.deployment_id,
                    "skipping auto dns rollback because another active deployment already exists"
                );
                Ok(0)
            }
            Err(error) => Err(error),
        }
    }

    async fn resolve_target_nodes(
        &self,
        cluster_id: &str,
        requested_target_node_ids: Option<&[String]>,
        allow_offline_targets: bool,
    ) -> ServiceResult<(Vec<rginx_control_types::NodeSummary>, usize)> {
        let all_nodes = self
            .store
            .node_repository()
            .list_nodes()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .into_iter()
            .filter(|node| node.cluster_id == cluster_id)
            .collect::<Vec<_>>();
        let eligible_online_nodes = all_nodes
            .iter()
            .filter(|node| {
                matches!(node.state, NodeLifecycleState::Online | NodeLifecycleState::Draining)
            })
            .cloned()
            .collect::<Vec<_>>();

        let selectable_nodes =
            if allow_offline_targets { all_nodes.clone() } else { eligible_online_nodes.clone() };

        if selectable_nodes.is_empty() {
            return Err(ServiceError::BadRequest(format!(
                "cluster `{cluster_id}` has no {} nodes eligible for dns deployment",
                if allow_offline_targets { "known" } else { "online or draining" }
            )));
        }

        let selected = if let Some(target_node_ids) = requested_target_node_ids {
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

            let by_id = selectable_nodes
                .into_iter()
                .map(|node| (node.node_id.clone(), node))
                .collect::<BTreeMap<_, _>>();
            let mut selected = Vec::with_capacity(target_node_ids.len());
            for node_id in target_node_ids {
                let Some(node) = by_id.get(node_id) else {
                    return Err(ServiceError::BadRequest(format!(
                        "node `{node_id}` is not eligible in cluster `{cluster_id}`"
                    )));
                };
                selected.push(node.clone());
            }
            selected
        } else {
            selectable_nodes
        };

        Ok((selected, eligible_online_nodes.len()))
    }

    async fn record_audit(&self, entry: NewAuditLogEntry) -> ServiceResult<()> {
        self.store
            .audit_repository()
            .insert_entry(&entry)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))
    }

    fn validate_create_request(&self, cluster_id: &str, revision_id: &str) -> ServiceResult<()> {
        if cluster_id.trim().is_empty() {
            return Err(ServiceError::BadRequest("cluster_id should not be empty".to_string()));
        }
        if revision_id.trim().is_empty() {
            return Err(ServiceError::BadRequest("revision_id should not be empty".to_string()));
        }
        Ok(())
    }

    fn generate_id(&self, prefix: &str) -> String {
        let now = unix_time_ms(SystemTime::now());
        let sequence = DNS_DEPLOYMENT_EVENT_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{prefix}_{now}_{sequence}")
    }
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

fn dispatch_capacity(parallelism: u32, active_nodes: u32) -> u32 {
    parallelism.saturating_sub(active_nodes)
}

fn should_block_new_dispatch(progress: &DnsDeploymentProgressSnapshot) -> bool {
    progress.failed_nodes > 0 && progress.failed_nodes >= progress.failure_threshold
}

fn should_finalize_now(progress: &DnsDeploymentProgressSnapshot) -> bool {
    progress.pending_nodes == 0 && progress.active_nodes == 0
}

fn is_target_confirmed(observation: &ActiveDnsDeploymentTargetObservation) -> bool {
    match (
        observation.assigned_at,
        observation.observed_at,
        observation.observed_revision_id.as_deref(),
    ) {
        (Some(assigned_at), Some(observed_at), Some(observed_revision_id))
            if observed_at >= assigned_at =>
        {
            observed_revision_id == observation.desired_revision_id
        }
        _ => false,
    }
}

fn unix_time_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().min(u128::from(u64::MAX)) as u64
}
