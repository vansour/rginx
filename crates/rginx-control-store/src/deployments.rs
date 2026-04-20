use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{Postgres, Row, Transaction};

use rginx_control_types::{
    ConfigRevisionSummary, DeploymentDetail, DeploymentStatus, DeploymentSummary,
    DeploymentTargetState, DeploymentTargetSummary, DeploymentTaskKind, DeploymentTaskState,
    NodeAgentTask, NodeAgentTaskAckResponse, NodeAgentTaskCompleteResponse,
};

use crate::repositories::{ControlPlaneStore, NewAuditLogEntry};

const DEPLOYMENT_EVENT_LIMIT: i64 = 20;

#[derive(Debug, Clone)]
pub struct CreateDeploymentRecord {
    pub deployment_id: String,
    pub cluster_id: String,
    pub revision_id: String,
    pub created_by: String,
    pub parallelism: u32,
    pub failure_threshold: u32,
    pub auto_rollback: bool,
    pub rollback_of_deployment_id: Option<String>,
    pub rollback_revision_id: Option<String>,
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct CreateDeploymentTargetRecord {
    pub target_id: String,
    pub cluster_id: String,
    pub node_id: String,
    pub desired_revision_id: String,
    pub batch_index: u32,
}

#[derive(Debug, Clone)]
pub struct DeploymentProgressSnapshot {
    pub deployment_id: String,
    pub cluster_id: String,
    pub status: DeploymentStatus,
    pub parallelism: u32,
    pub failure_threshold: u32,
    pub auto_rollback: bool,
    pub rollback_of_deployment_id: Option<String>,
    pub rollback_revision_id: Option<String>,
    pub total_nodes: u32,
    pub pending_nodes: u32,
    pub in_flight_nodes: u32,
    pub healthy_nodes: u32,
    pub failed_nodes: u32,
}

#[derive(Debug, Clone)]
pub struct TaskCompletionRecord {
    pub succeeded: bool,
    pub message: Option<String>,
    pub runtime_revision: Option<u64>,
    pub completion_idempotency_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DeploymentRepository {
    store: ControlPlaneStore,
}

impl DeploymentRepository {
    pub fn new(store: ControlPlaneStore) -> Self {
        Self { store }
    }

    pub async fn find_id_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<String>> {
        sqlx::query_scalar::<_, String>(
            r#"
            select deployment_id
            from cp_deployments
            where idempotency_key = $1
            "#,
        )
        .bind(idempotency_key)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| {
            format!("failed to load deployment by idempotency key `{idempotency_key}`")
        })
    }

    pub async fn find_active_cluster_deployment(
        &self,
        cluster_id: &str,
    ) -> Result<Option<DeploymentSummary>> {
        sqlx::query(
            &deployment_summary_select(
                "where d.cluster_id = $1 and d.status in ('running', 'paused') order by d.created_at asc limit 1",
            ),
        )
        .bind(cluster_id)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| format!("failed to load active deployment for cluster `{cluster_id}`"))?
        .map(map_deployment_summary_row)
        .transpose()
    }

    pub async fn list_deployments(&self) -> Result<Vec<DeploymentSummary>> {
        sqlx::query(&deployment_summary_select("order by d.created_at desc, d.deployment_id desc"))
            .fetch_all(self.store.postgres())
            .await
            .context("failed to list deployments from postgres")?
            .into_iter()
            .map(map_deployment_summary_row)
            .collect()
    }

    pub async fn load_deployment_detail(
        &self,
        deployment_id: &str,
    ) -> Result<Option<DeploymentDetail>> {
        let Some(deployment) = self.load_deployment_summary(deployment_id).await? else {
            return Ok(None);
        };
        let revision =
            self.load_revision_summary(&deployment.revision_id).await?.ok_or_else(|| {
                anyhow!(
                    "revision `{}` for deployment `{deployment_id}` disappeared",
                    deployment.revision_id
                )
            })?;
        let rollback_revision = match deployment.rollback_revision_id.as_deref() {
            Some(revision_id) => self.load_revision_summary(revision_id).await?,
            None => None,
        };
        let targets = sqlx::query(
            r#"
            select
                t.target_id,
                t.deployment_id,
                t.node_id,
                n.advertise_addr,
                n.state as node_state,
                t.desired_revision_id,
                t.state,
                t.task_id,
                at.kind as task_kind,
                at.state as task_state,
                t.attempt_count,
                t.batch_index,
                t.last_error,
                t.dispatched_at,
                t.acked_at,
                t.completed_at
            from cp_deployment_targets t
            join cp_nodes n on n.node_id = t.node_id
            left join cp_agent_tasks at on at.target_id = t.target_id
            where t.deployment_id = $1
            order by t.batch_index asc, t.node_id asc
            "#,
        )
        .bind(deployment_id)
        .fetch_all(self.store.postgres())
        .await
        .with_context(|| format!("failed to load deployment targets for `{deployment_id}`"))?
        .into_iter()
        .map(map_deployment_target_row)
        .collect::<Result<Vec<_>>>()?;
        let recent_events = self
            .store
            .audit_repository()
            .list_timeline_for_deployment(deployment_id, DEPLOYMENT_EVENT_LIMIT)
            .await
            .with_context(|| {
                format!("failed to load deployment audit trail for `{deployment_id}`")
            })?;

        Ok(Some(DeploymentDetail {
            deployment,
            revision,
            rollback_revision,
            targets,
            recent_events,
        }))
    }

    pub async fn create_deployment_with_audit(
        &self,
        record: &CreateDeploymentRecord,
        targets: &[CreateDeploymentTargetRecord],
        audit: &NewAuditLogEntry,
    ) -> Result<DeploymentDetail> {
        let mut transaction = self
            .store
            .postgres()
            .begin()
            .await
            .context("failed to start deployment creation transaction")?;

        sqlx::query(
            r#"
            insert into cp_deployments (
                deployment_id,
                cluster_id,
                revision_id,
                status,
                target_nodes,
                healthy_nodes,
                created_at,
                finished_at,
                started_by,
                created_by,
                parallelism,
                failure_threshold,
                auto_rollback,
                rollback_of_deployment_id,
                rollback_revision_id,
                status_reason,
                idempotency_key,
                started_at,
                updated_at
            )
            values (
                $1,
                $2,
                $3,
                'running',
                $4,
                0,
                $5,
                null,
                $6,
                $6,
                $7,
                $8,
                $9,
                $10,
                $11,
                null,
                $12,
                $5,
                $5
            )
            "#,
        )
        .bind(&record.deployment_id)
        .bind(&record.cluster_id)
        .bind(&record.revision_id)
        .bind(i32::try_from(targets.len()).context("target count should fit into i32")?)
        .bind(record.created_at)
        .bind(&record.created_by)
        .bind(i32::try_from(record.parallelism).context("parallelism should fit into i32")?)
        .bind(
            i32::try_from(record.failure_threshold)
                .context("failure_threshold should fit into i32")?,
        )
        .bind(record.auto_rollback)
        .bind(&record.rollback_of_deployment_id)
        .bind(&record.rollback_revision_id)
        .bind(&record.idempotency_key)
        .execute(&mut *transaction)
        .await
        .with_context(|| format!("failed to insert deployment `{}`", record.deployment_id))?;

        for target in targets {
            sqlx::query(
                r#"
                insert into cp_deployment_targets (
                    target_id,
                    deployment_id,
                    cluster_id,
                    node_id,
                    desired_revision_id,
                    state,
                    task_id,
                    attempt_count,
                    batch_index,
                    last_error,
                    dispatched_at,
                    acked_at,
                    completed_at,
                    created_at,
                    updated_at
                )
                values (
                    $1,
                    $2,
                    $3,
                    $4,
                    $5,
                    'pending',
                    null,
                    0,
                    $6,
                    null,
                    null,
                    null,
                    null,
                    $7,
                    $7
                )
                "#,
            )
            .bind(&target.target_id)
            .bind(&record.deployment_id)
            .bind(&target.cluster_id)
            .bind(&target.node_id)
            .bind(&target.desired_revision_id)
            .bind(i32::try_from(target.batch_index).context("batch_index should fit into i32")?)
            .bind(record.created_at)
            .execute(&mut *transaction)
            .await
            .with_context(|| {
                format!(
                    "failed to insert deployment target `{}` for deployment `{}`",
                    target.target_id, record.deployment_id
                )
            })?;
        }

        insert_audit_entry(&mut transaction, audit).await?;
        transaction.commit().await.context("failed to commit deployment creation transaction")?;

        self.load_deployment_detail(&record.deployment_id).await?.ok_or_else(|| {
            anyhow!("deployment `{}` disappeared after create commit", record.deployment_id)
        })
    }

    pub async fn list_active_deployment_ids(&self) -> Result<Vec<String>> {
        sqlx::query_scalar::<_, String>(
            r#"
            select deployment_id
            from cp_deployments
            where status in ('running', 'paused')
            order by created_at asc, deployment_id asc
            "#,
        )
        .fetch_all(self.store.postgres())
        .await
        .context("failed to list active deployments from postgres")
    }

    pub async fn load_progress_snapshot(
        &self,
        deployment_id: &str,
    ) -> Result<Option<DeploymentProgressSnapshot>> {
        sqlx::query(
            r#"
            select
                d.deployment_id,
                d.cluster_id,
                d.status,
                d.parallelism,
                d.failure_threshold,
                d.auto_rollback,
                d.rollback_of_deployment_id,
                d.rollback_revision_id,
                count(t.target_id) as total_nodes,
                count(*) filter (where t.state = 'pending') as pending_nodes,
                count(*) filter (where t.state in ('dispatched', 'acknowledged')) as in_flight_nodes,
                count(*) filter (where t.state = 'succeeded') as healthy_nodes,
                count(*) filter (where t.state = 'failed') as failed_nodes
            from cp_deployments d
            left join cp_deployment_targets t on t.deployment_id = d.deployment_id
            where d.deployment_id = $1
            group by
                d.deployment_id,
                d.cluster_id,
                d.status,
                d.parallelism,
                d.failure_threshold,
                d.auto_rollback,
                d.rollback_of_deployment_id,
                d.rollback_revision_id
            "#,
        )
        .bind(deployment_id)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| format!("failed to load deployment progress for `{deployment_id}`"))?
        .map(map_progress_row)
        .transpose()
    }

    pub async fn dispatch_next_targets(&self, deployment_id: &str, limit: u32) -> Result<u32> {
        if limit == 0 {
            return Ok(0);
        }

        let mut transaction = self
            .store
            .postgres()
            .begin()
            .await
            .context("failed to start deployment dispatch transaction")?;
        let deployment = sqlx::query(
            r#"
            select cluster_id, rollback_of_deployment_id, revision_id
            from cp_deployments
            where deployment_id = $1
            for update
            "#,
        )
        .bind(deployment_id)
        .fetch_optional(&mut *transaction)
        .await
        .with_context(|| format!("failed to lock deployment `{deployment_id}` for dispatch"))?;
        let Some(deployment) = deployment else {
            transaction
                .rollback()
                .await
                .context("failed to rollback missing deployment dispatch transaction")?;
            return Ok(0);
        };
        let cluster_id: String =
            deployment.try_get("cluster_id").context("cluster_id should be present")?;
        let revision_id: String =
            deployment.try_get("revision_id").context("revision_id should be present")?;
        let task_kind = if deployment
            .try_get::<Option<String>, _>("rollback_of_deployment_id")
            .context("rollback_of_deployment_id should be readable")?
            .is_some()
        {
            DeploymentTaskKind::RollbackRevision
        } else {
            DeploymentTaskKind::ApplyRevision
        };
        let revision = sqlx::query(
            r#"
            select source_path, config_text
            from cp_config_revisions
            where revision_id = $1
            "#,
        )
        .bind(&revision_id)
        .fetch_one(&mut *transaction)
        .await
        .with_context(|| format!("failed to load revision `{revision_id}` for dispatch"))?;
        let source_path: String =
            revision.try_get("source_path").context("source_path should be present")?;
        let config_text: String =
            revision.try_get("config_text").context("config_text should be present")?;
        let pending_targets = sqlx::query(
            r#"
            select target_id, node_id, desired_revision_id, attempt_count
            from cp_deployment_targets
            where deployment_id = $1
              and state = 'pending'
            order by batch_index asc, node_id asc
            limit $2
            for update skip locked
            "#,
        )
        .bind(deployment_id)
        .bind(i64::from(limit))
        .fetch_all(&mut *transaction)
        .await
        .with_context(|| {
            format!("failed to select pending deployment targets for `{deployment_id}`")
        })?;

        for (index, target) in pending_targets.iter().enumerate() {
            let target_id: String =
                target.try_get("target_id").context("target_id should be present")?;
            let node_id: String = target.try_get("node_id").context("node_id should be present")?;
            let attempt_count: i32 =
                target.try_get("attempt_count").context("attempt_count should be present")?;
            let task_id =
                format!("task_{}_{}_{}", Utc::now().timestamp_millis(), index + 1, target_id);
            let next_attempt = attempt_count + 1;

            sqlx::query(
                r#"
                insert into cp_agent_tasks (
                    task_id,
                    deployment_id,
                    target_id,
                    cluster_id,
                    node_id,
                    kind,
                    state,
                    revision_id,
                    source_path,
                    config_text,
                    attempt,
                    completion_idempotency_key,
                    result_message,
                    result_payload,
                    dispatched_at,
                    acked_at,
                    completed_at,
                    updated_at
                )
                values (
                    $1,
                    $2,
                    $3,
                    $4,
                    $5,
                    $6,
                    'pending',
                    $7,
                    $8,
                    $9,
                    $10,
                    null,
                    null,
                    '{}'::jsonb,
                    now(),
                    null,
                    null,
                    now()
                )
                "#,
            )
            .bind(&task_id)
            .bind(deployment_id)
            .bind(&target_id)
            .bind(&cluster_id)
            .bind(&node_id)
            .bind(task_kind.as_str())
            .bind(&revision_id)
            .bind(&source_path)
            .bind(&config_text)
            .bind(next_attempt)
            .execute(&mut *transaction)
            .await
            .with_context(|| {
                format!("failed to insert task `{task_id}` for deployment target `{target_id}`")
            })?;

            sqlx::query(
                r#"
                update cp_deployment_targets
                set state = 'dispatched',
                    task_id = $2,
                    attempt_count = $3,
                    last_error = null,
                    dispatched_at = now(),
                    updated_at = now()
                where target_id = $1
                "#,
            )
            .bind(&target_id)
            .bind(&task_id)
            .bind(next_attempt)
            .execute(&mut *transaction)
            .await
            .with_context(|| {
                format!("failed to mark deployment target `{target_id}` as dispatched")
            })?;
        }

        sqlx::query(
            r#"
            update cp_deployments
            set updated_at = now()
            where deployment_id = $1
            "#,
        )
        .bind(deployment_id)
        .execute(&mut *transaction)
        .await
        .with_context(|| format!("failed to bump updated_at for deployment `{deployment_id}`"))?;

        transaction.commit().await.context("failed to commit deployment dispatch transaction")?;

        count_to_u32(
            i64::try_from(pending_targets.len()).context("dispatch size should fit into i64")?,
            "dispatched_targets",
        )
    }

    pub async fn cancel_pending_targets(&self, deployment_id: &str, reason: &str) -> Result<u32> {
        let rows_affected = sqlx::query(
            r#"
            update cp_deployment_targets
            set state = 'cancelled',
                last_error = $2,
                completed_at = coalesce(completed_at, now()),
                updated_at = now()
            where deployment_id = $1
              and state = 'pending'
            "#,
        )
        .bind(deployment_id)
        .bind(reason)
        .execute(self.store.postgres())
        .await
        .with_context(|| format!("failed to cancel pending targets for `{deployment_id}`"))?
        .rows_affected();

        count_to_u32(
            i64::try_from(rows_affected).context("rows_affected should fit into i64")?,
            "cancelled_targets",
        )
    }

    pub async fn set_status_reason(&self, deployment_id: &str, reason: Option<&str>) -> Result<()> {
        sqlx::query(
            r#"
            update cp_deployments
            set status_reason = $2,
                updated_at = now()
            where deployment_id = $1
            "#,
        )
        .bind(deployment_id)
        .bind(reason)
        .execute(self.store.postgres())
        .await
        .with_context(|| {
            format!("failed to update status_reason for deployment `{deployment_id}`")
        })?;
        Ok(())
    }

    pub async fn set_deployment_paused(
        &self,
        deployment_id: &str,
        paused: bool,
    ) -> Result<Option<DeploymentSummary>> {
        sqlx::query(
            r#"
            update cp_deployments
            set status = $2,
                status_reason = $3,
                updated_at = now()
            where deployment_id = $1
            "#,
        )
        .bind(deployment_id)
        .bind(if paused { "paused" } else { "running" })
        .bind(if paused { Some("paused by administrator") } else { None })
        .execute(self.store.postgres())
        .await
        .with_context(|| {
            format!("failed to update pause state for deployment `{deployment_id}`")
        })?;

        self.load_deployment_summary(deployment_id).await
    }

    pub async fn mark_deployment_succeeded(
        &self,
        deployment_id: &str,
        reason: Option<&str>,
    ) -> Result<Option<DeploymentSummary>> {
        self.mark_deployment_terminal_status(deployment_id, DeploymentStatus::Succeeded, reason)
            .await
    }

    pub async fn mark_deployment_failed(
        &self,
        deployment_id: &str,
        reason: &str,
    ) -> Result<Option<DeploymentSummary>> {
        self.mark_deployment_terminal_status(deployment_id, DeploymentStatus::Failed, Some(reason))
            .await
    }

    pub async fn mark_deployment_rolled_back(
        &self,
        deployment_id: &str,
        reason: &str,
    ) -> Result<Option<DeploymentSummary>> {
        sqlx::query(
            r#"
            update cp_deployments
            set status = 'rolled_back',
                status_reason = $2,
                updated_at = now(),
                finished_at = coalesce(finished_at, now())
            where deployment_id = $1
            "#,
        )
        .bind(deployment_id)
        .bind(reason)
        .execute(self.store.postgres())
        .await
        .with_context(|| format!("failed to mark deployment `{deployment_id}` as rolled back"))?;

        self.load_deployment_summary(deployment_id).await
    }

    pub async fn find_rollback_child(
        &self,
        deployment_id: &str,
    ) -> Result<Option<DeploymentSummary>> {
        sqlx::query(&deployment_summary_select(
            "where d.rollback_of_deployment_id = $1 order by d.created_at desc limit 1",
        ))
        .bind(deployment_id)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| format!("failed to load rollback child for deployment `{deployment_id}`"))?
        .map(map_deployment_summary_row)
        .transpose()
    }

    pub async fn list_succeeded_target_node_ids(&self, deployment_id: &str) -> Result<Vec<String>> {
        sqlx::query_scalar::<_, String>(
            r#"
            select node_id
            from cp_deployment_targets
            where deployment_id = $1
              and state = 'succeeded'
            order by batch_index asc, node_id asc
            "#,
        )
        .bind(deployment_id)
        .fetch_all(self.store.postgres())
        .await
        .with_context(|| {
            format!("failed to load succeeded target nodes for deployment `{deployment_id}`")
        })
    }

    pub async fn poll_task_for_node(
        &self,
        node_id: &str,
        cluster_id: &str,
    ) -> Result<Option<NodeAgentTask>> {
        sqlx::query(
            r#"
            select
                t.task_id,
                t.deployment_id,
                t.target_id,
                t.cluster_id,
                t.node_id,
                t.kind,
                t.state,
                t.revision_id,
                r.version_label as revision_version_label,
                t.source_path,
                t.config_text,
                t.attempt,
                t.dispatched_at
            from cp_agent_tasks t
            join cp_config_revisions r on r.revision_id = t.revision_id
            join cp_deployments d on d.deployment_id = t.deployment_id
            where t.node_id = $1
              and t.cluster_id = $2
              and t.state in ('pending', 'acknowledged')
              and d.status in ('running', 'paused')
            order by
                case t.state when 'acknowledged' then 0 else 1 end asc,
                t.dispatched_at asc,
                t.task_id asc
            limit 1
            "#,
        )
        .bind(node_id)
        .bind(cluster_id)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| format!("failed to poll task for node `{node_id}`"))?
        .map(map_agent_task_row)
        .transpose()
    }

    pub async fn ack_task(
        &self,
        task_id: &str,
        node_id: &str,
    ) -> Result<Option<NodeAgentTaskAckResponse>> {
        let mut transaction =
            self.store.postgres().begin().await.context("failed to start task ack transaction")?;
        let task = sqlx::query(
            r#"
            select task_id, deployment_id, state, dispatched_at, acked_at, completed_at, target_id
            from cp_agent_tasks
            where task_id = $1
              and node_id = $2
            for update
            "#,
        )
        .bind(task_id)
        .bind(node_id)
        .fetch_optional(&mut *transaction)
        .await
        .with_context(|| format!("failed to lock task `{task_id}` for ack"))?;
        let Some(task) = task else {
            transaction
                .rollback()
                .await
                .context("failed to rollback missing task ack transaction")?;
            return Ok(None);
        };
        let state: String = task.try_get("state").context("task state should be present")?;
        let deployment_id: String =
            task.try_get("deployment_id").context("deployment_id should be present")?;
        let target_id: String = task.try_get("target_id").context("target_id should be present")?;

        if state == "pending" {
            sqlx::query(
                r#"
                update cp_agent_tasks
                set state = 'acknowledged',
                    acked_at = now(),
                    updated_at = now()
                where task_id = $1
                "#,
            )
            .bind(task_id)
            .execute(&mut *transaction)
            .await
            .with_context(|| format!("failed to acknowledge task `{task_id}`"))?;

            sqlx::query(
                r#"
                update cp_deployment_targets
                set state = 'acknowledged',
                    acked_at = now(),
                    updated_at = now()
                where target_id = $1
                "#,
            )
            .bind(&target_id)
            .execute(&mut *transaction)
            .await
            .with_context(|| format!("failed to acknowledge deployment target `{target_id}`"))?;
        }

        transaction.commit().await.context("failed to commit task ack transaction")?;

        let acknowledged_at = sqlx::query(
            r#"
            select state, dispatched_at, acked_at, completed_at
            from cp_agent_tasks
            where task_id = $1
            "#,
        )
        .bind(task_id)
        .fetch_one(self.store.postgres())
        .await
        .with_context(|| format!("failed to reload task `{task_id}` after ack"))?;
        let final_state = acknowledged_at
            .try_get::<String, _>("state")
            .context("task state should be present")?;
        let acked_at: Option<DateTime<Utc>> =
            acknowledged_at.try_get("acked_at").context("acked_at should be readable")?;
        let completed_at: Option<DateTime<Utc>> =
            acknowledged_at.try_get("completed_at").context("completed_at should be readable")?;
        let dispatched_at: DateTime<Utc> =
            acknowledged_at.try_get("dispatched_at").context("dispatched_at should be present")?;

        Ok(Some(NodeAgentTaskAckResponse {
            task_id: task_id.to_string(),
            deployment_id,
            state: final_state.parse().map_err(|error: String| anyhow!(error)).with_context(
                || format!("invalid task state `{final_state}` loaded from postgres"),
            )?,
            acknowledged_at_unix_ms: unix_time_ms(
                acked_at.or(completed_at).unwrap_or(dispatched_at),
            )?,
            agent_token: None,
            agent_token_expires_at_unix_ms: None,
        }))
    }

    pub async fn complete_task(
        &self,
        task_id: &str,
        node_id: &str,
        completion: &TaskCompletionRecord,
    ) -> Result<Option<NodeAgentTaskCompleteResponse>> {
        let mut transaction = self
            .store
            .postgres()
            .begin()
            .await
            .context("failed to start task completion transaction")?;
        let task = sqlx::query(
            r#"
            select
                task_id,
                deployment_id,
                target_id,
                state,
                dispatched_at,
                completed_at
            from cp_agent_tasks
            where task_id = $1
              and node_id = $2
            for update
            "#,
        )
        .bind(task_id)
        .bind(node_id)
        .fetch_optional(&mut *transaction)
        .await
        .with_context(|| format!("failed to lock task `{task_id}` for completion"))?;
        let Some(task) = task else {
            transaction
                .rollback()
                .await
                .context("failed to rollback missing task completion transaction")?;
            return Ok(None);
        };
        let current_state: String =
            task.try_get("state").context("task state should be present")?;
        let deployment_id: String =
            task.try_get("deployment_id").context("deployment_id should be present")?;
        let target_id: String = task.try_get("target_id").context("target_id should be present")?;

        if !matches!(current_state.as_str(), "succeeded" | "failed" | "cancelled") {
            let next_state = if completion.succeeded {
                DeploymentTaskState::Succeeded
            } else {
                DeploymentTaskState::Failed
            };
            let target_state = if completion.succeeded {
                DeploymentTargetState::Succeeded
            } else {
                DeploymentTargetState::Failed
            };
            let result_payload = json!({
                "runtime_revision": completion.runtime_revision,
            });

            sqlx::query(
                r#"
                update cp_agent_tasks
                set state = $2,
                    completion_idempotency_key = coalesce($3, completion_idempotency_key),
                    result_message = $4,
                    result_payload = $5,
                    acked_at = coalesce(acked_at, now()),
                    completed_at = now(),
                    updated_at = now()
                where task_id = $1
                "#,
            )
            .bind(task_id)
            .bind(next_state.as_str())
            .bind(&completion.completion_idempotency_key)
            .bind(&completion.message)
            .bind(result_payload)
            .execute(&mut *transaction)
            .await
            .with_context(|| format!("failed to complete task `{task_id}`"))?;

            sqlx::query(
                r#"
                update cp_deployment_targets
                set state = $2,
                    acked_at = coalesce(acked_at, now()),
                    completed_at = now(),
                    last_error = $3,
                    updated_at = now()
                where target_id = $1
                "#,
            )
            .bind(&target_id)
            .bind(target_state.as_str())
            .bind(if completion.succeeded { None } else { completion.message.clone() })
            .execute(&mut *transaction)
            .await
            .with_context(|| format!("failed to complete deployment target `{target_id}`"))?;

            sqlx::query(
                r#"
                update cp_deployments
                set updated_at = now()
                where deployment_id = $1
                "#,
            )
            .bind(&deployment_id)
            .execute(&mut *transaction)
            .await
            .with_context(|| {
                format!("failed to bump updated_at after task completion for `{deployment_id}`")
            })?;
        }

        transaction.commit().await.context("failed to commit task completion transaction")?;

        let row = sqlx::query(
            r#"
            select state, dispatched_at, completed_at
            from cp_agent_tasks
            where task_id = $1
            "#,
        )
        .bind(task_id)
        .fetch_one(self.store.postgres())
        .await
        .with_context(|| format!("failed to reload task `{task_id}` after completion"))?;
        let final_state =
            row.try_get::<String, _>("state").context("task state should be present")?;
        let dispatched_at: DateTime<Utc> =
            row.try_get("dispatched_at").context("dispatched_at should be present")?;
        let completed_at: Option<DateTime<Utc>> =
            row.try_get("completed_at").context("completed_at should be readable")?;

        Ok(Some(NodeAgentTaskCompleteResponse {
            task_id: task_id.to_string(),
            deployment_id,
            state: final_state.parse().map_err(|error: String| anyhow!(error)).with_context(
                || format!("invalid task state `{final_state}` loaded from postgres"),
            )?,
            completed_at_unix_ms: unix_time_ms(completed_at.unwrap_or(dispatched_at))?,
            agent_token: None,
            agent_token_expires_at_unix_ms: None,
        }))
    }

    async fn load_deployment_summary(
        &self,
        deployment_id: &str,
    ) -> Result<Option<DeploymentSummary>> {
        sqlx::query(&deployment_summary_select("where d.deployment_id = $1"))
            .bind(deployment_id)
            .fetch_optional(self.store.postgres())
            .await
            .with_context(|| format!("failed to load deployment `{deployment_id}`"))?
            .map(map_deployment_summary_row)
            .transpose()
    }

    async fn load_revision_summary(
        &self,
        revision_id: &str,
    ) -> Result<Option<ConfigRevisionSummary>> {
        sqlx::query(
            r#"
            select revision_id, cluster_id, version_label, summary, created_at
            from cp_config_revisions
            where revision_id = $1
            "#,
        )
        .bind(revision_id)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| format!("failed to load revision summary `{revision_id}`"))?
        .map(map_revision_summary_row)
        .transpose()
    }

    async fn mark_deployment_terminal_status(
        &self,
        deployment_id: &str,
        status: DeploymentStatus,
        reason: Option<&str>,
    ) -> Result<Option<DeploymentSummary>> {
        sqlx::query(
            r#"
            update cp_deployments
            set status = $2,
                status_reason = $3,
                finished_at = now(),
                updated_at = now()
            where deployment_id = $1
            "#,
        )
        .bind(deployment_id)
        .bind(status.as_str())
        .bind(reason)
        .execute(self.store.postgres())
        .await
        .with_context(|| {
            format!("failed to mark deployment `{deployment_id}` as `{}`", status.as_str())
        })?;

        self.load_deployment_summary(deployment_id).await
    }
}

fn deployment_summary_select(suffix: &str) -> String {
    format!(
        r#"
        with target_stats as (
            select
                deployment_id,
                count(*)::bigint as target_nodes,
                count(*) filter (where state = 'succeeded')::bigint as healthy_nodes,
                count(*) filter (where state = 'failed')::bigint as failed_nodes,
                count(*) filter (where state in ('dispatched', 'acknowledged'))::bigint as in_flight_nodes
            from cp_deployment_targets
            group by deployment_id
        )
        select
            d.deployment_id,
            d.cluster_id,
            d.revision_id,
            r.version_label as revision_version_label,
            d.status,
            coalesce(ts.target_nodes, d.target_nodes::bigint) as target_nodes,
            coalesce(ts.healthy_nodes, d.healthy_nodes::bigint) as healthy_nodes,
            coalesce(ts.failed_nodes, 0::bigint) as failed_nodes,
            coalesce(ts.in_flight_nodes, 0::bigint) as in_flight_nodes,
            d.parallelism,
            d.failure_threshold,
            d.auto_rollback,
            d.created_by,
            d.rollback_of_deployment_id,
            d.rollback_revision_id,
            d.status_reason,
            d.created_at,
            d.started_at,
            d.finished_at
        from cp_deployments d
        join cp_config_revisions r on r.revision_id = d.revision_id
        left join target_stats ts on ts.deployment_id = d.deployment_id
        {suffix}
        "#
    )
}

async fn insert_audit_entry(
    transaction: &mut Transaction<'_, Postgres>,
    entry: &NewAuditLogEntry,
) -> Result<()> {
    sqlx::query(
        r#"
        insert into cp_audit_logs (
            audit_id,
            request_id,
            cluster_id,
            actor_id,
            action,
            resource_type,
            resource_id,
            result,
            details,
            created_at
        )
        values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        "#,
    )
    .bind(&entry.audit_id)
    .bind(&entry.request_id)
    .bind(&entry.cluster_id)
    .bind(&entry.actor_id)
    .bind(&entry.action)
    .bind(&entry.resource_type)
    .bind(&entry.resource_id)
    .bind(&entry.result)
    .bind(&entry.details)
    .bind(entry.created_at)
    .execute(&mut **transaction)
    .await
    .with_context(|| format!("failed to insert control-plane audit log `{}`", entry.audit_id))?;
    Ok(())
}

fn map_revision_summary_row(row: sqlx::postgres::PgRow) -> Result<ConfigRevisionSummary> {
    Ok(ConfigRevisionSummary {
        revision_id: row.try_get("revision_id").context("revision_id should be present")?,
        cluster_id: row.try_get("cluster_id").context("cluster_id should be present")?,
        version_label: row.try_get("version_label").context("version_label should be present")?,
        summary: row.try_get("summary").context("summary should be present")?,
        created_at_unix_ms: unix_time_ms(
            row.try_get::<DateTime<Utc>, _>("created_at")
                .context("created_at should be present")?,
        )?,
    })
}

fn map_deployment_summary_row(row: sqlx::postgres::PgRow) -> Result<DeploymentSummary> {
    let status =
        row.try_get::<String, _>("status").context("deployment status should be present")?;
    let parallelism: i32 = row.try_get("parallelism").context("parallelism should be present")?;
    let failure_threshold: i32 =
        row.try_get("failure_threshold").context("failure_threshold should be present")?;
    let started_at: Option<DateTime<Utc>> =
        row.try_get("started_at").context("started_at should be readable")?;
    let finished_at: Option<DateTime<Utc>> =
        row.try_get("finished_at").context("finished_at should be readable")?;

    Ok(DeploymentSummary {
        deployment_id: row.try_get("deployment_id").context("deployment_id should be present")?,
        cluster_id: row.try_get("cluster_id").context("cluster_id should be present")?,
        revision_id: row.try_get("revision_id").context("revision_id should be present")?,
        revision_version_label: row
            .try_get("revision_version_label")
            .context("revision_version_label should be present")?,
        status: status.parse().map_err(|error: String| anyhow!(error)).with_context(|| {
            format!("invalid deployment status `{status}` loaded from postgres")
        })?,
        target_nodes: count_to_u32(
            row.try_get::<i64, _>("target_nodes").context("target_nodes should be present")?,
            "target_nodes",
        )?,
        healthy_nodes: count_to_u32(
            row.try_get::<i64, _>("healthy_nodes").context("healthy_nodes should be present")?,
            "healthy_nodes",
        )?,
        failed_nodes: count_to_u32(
            row.try_get::<i64, _>("failed_nodes").context("failed_nodes should be present")?,
            "failed_nodes",
        )?,
        in_flight_nodes: count_to_u32(
            row.try_get::<i64, _>("in_flight_nodes")
                .context("in_flight_nodes should be present")?,
            "in_flight_nodes",
        )?,
        parallelism: u32::try_from(parallelism).context("parallelism should fit into u32")?,
        failure_threshold: u32::try_from(failure_threshold)
            .context("failure_threshold should fit into u32")?,
        auto_rollback: row.try_get("auto_rollback").context("auto_rollback should be present")?,
        created_by: row.try_get("created_by").context("created_by should be present")?,
        rollback_of_deployment_id: row
            .try_get("rollback_of_deployment_id")
            .context("rollback_of_deployment_id should be readable")?,
        rollback_revision_id: row
            .try_get("rollback_revision_id")
            .context("rollback_revision_id should be readable")?,
        status_reason: row.try_get("status_reason").context("status_reason should be readable")?,
        created_at_unix_ms: unix_time_ms(
            row.try_get::<DateTime<Utc>, _>("created_at")
                .context("created_at should be present")?,
        )?,
        started_at_unix_ms: started_at.map(unix_time_ms).transpose()?,
        finished_at_unix_ms: finished_at.map(unix_time_ms).transpose()?,
    })
}

fn map_deployment_target_row(row: sqlx::postgres::PgRow) -> Result<DeploymentTargetSummary> {
    let state = row.try_get::<String, _>("state").context("target state should be present")?;
    let node_state =
        row.try_get::<String, _>("node_state").context("node_state should be present")?;
    let task_kind: Option<String> =
        row.try_get("task_kind").context("task_kind should be readable")?;
    let task_state: Option<String> =
        row.try_get("task_state").context("task_state should be readable")?;
    let attempt_count: i32 =
        row.try_get("attempt_count").context("attempt_count should be present")?;
    let batch_index: i32 = row.try_get("batch_index").context("batch_index should be present")?;
    let dispatched_at: Option<DateTime<Utc>> =
        row.try_get("dispatched_at").context("dispatched_at should be readable")?;
    let acked_at: Option<DateTime<Utc>> =
        row.try_get("acked_at").context("acked_at should be readable")?;
    let completed_at: Option<DateTime<Utc>> =
        row.try_get("completed_at").context("completed_at should be readable")?;

    Ok(DeploymentTargetSummary {
        target_id: row.try_get("target_id").context("target_id should be present")?,
        deployment_id: row.try_get("deployment_id").context("deployment_id should be present")?,
        node_id: row.try_get("node_id").context("node_id should be present")?,
        advertise_addr: row
            .try_get("advertise_addr")
            .context("advertise_addr should be present")?,
        node_state: node_state
            .parse()
            .map_err(|error: String| anyhow!(error))
            .with_context(|| format!("invalid node state `{node_state}` loaded from postgres"))?,
        desired_revision_id: row
            .try_get("desired_revision_id")
            .context("desired_revision_id should be present")?,
        state: state.parse().map_err(|error: String| anyhow!(error)).with_context(|| {
            format!("invalid deployment target state `{state}` loaded from postgres")
        })?,
        task_id: row.try_get("task_id").context("task_id should be readable")?,
        task_kind: task_kind
            .map(|value| {
                value.parse().map_err(|error: String| anyhow!(error)).with_context(|| {
                    format!("invalid deployment task kind `{value}` loaded from postgres")
                })
            })
            .transpose()?,
        task_state: task_state
            .map(|value| {
                value.parse().map_err(|error: String| anyhow!(error)).with_context(|| {
                    format!("invalid deployment task state `{value}` loaded from postgres")
                })
            })
            .transpose()?,
        attempt: u32::try_from(attempt_count).context("attempt_count should fit into u32")?,
        batch_index: u32::try_from(batch_index).context("batch_index should fit into u32")?,
        last_error: row.try_get("last_error").context("last_error should be readable")?,
        dispatched_at_unix_ms: dispatched_at.map(unix_time_ms).transpose()?,
        acked_at_unix_ms: acked_at.map(unix_time_ms).transpose()?,
        completed_at_unix_ms: completed_at.map(unix_time_ms).transpose()?,
    })
}

fn map_progress_row(row: sqlx::postgres::PgRow) -> Result<DeploymentProgressSnapshot> {
    let status =
        row.try_get::<String, _>("status").context("deployment status should be present")?;
    let parallelism: i32 = row.try_get("parallelism").context("parallelism should be present")?;
    let failure_threshold: i32 =
        row.try_get("failure_threshold").context("failure_threshold should be present")?;

    Ok(DeploymentProgressSnapshot {
        deployment_id: row.try_get("deployment_id").context("deployment_id should be present")?,
        cluster_id: row.try_get("cluster_id").context("cluster_id should be present")?,
        status: status.parse().map_err(|error: String| anyhow!(error)).with_context(|| {
            format!("invalid deployment status `{status}` loaded from postgres")
        })?,
        parallelism: u32::try_from(parallelism).context("parallelism should fit into u32")?,
        failure_threshold: u32::try_from(failure_threshold)
            .context("failure_threshold should fit into u32")?,
        auto_rollback: row.try_get("auto_rollback").context("auto_rollback should be present")?,
        rollback_of_deployment_id: row
            .try_get("rollback_of_deployment_id")
            .context("rollback_of_deployment_id should be readable")?,
        rollback_revision_id: row
            .try_get("rollback_revision_id")
            .context("rollback_revision_id should be readable")?,
        total_nodes: count_to_u32(
            row.try_get::<i64, _>("total_nodes").context("total_nodes should be present")?,
            "total_nodes",
        )?,
        pending_nodes: count_to_u32(
            row.try_get::<i64, _>("pending_nodes").context("pending_nodes should be present")?,
            "pending_nodes",
        )?,
        in_flight_nodes: count_to_u32(
            row.try_get::<i64, _>("in_flight_nodes")
                .context("in_flight_nodes should be present")?,
            "in_flight_nodes",
        )?,
        healthy_nodes: count_to_u32(
            row.try_get::<i64, _>("healthy_nodes").context("healthy_nodes should be present")?,
            "healthy_nodes",
        )?,
        failed_nodes: count_to_u32(
            row.try_get::<i64, _>("failed_nodes").context("failed_nodes should be present")?,
            "failed_nodes",
        )?,
    })
}

fn map_agent_task_row(row: sqlx::postgres::PgRow) -> Result<NodeAgentTask> {
    let kind = row.try_get::<String, _>("kind").context("task kind should be present")?;
    let state = row.try_get::<String, _>("state").context("task state should be present")?;
    let attempt: i32 = row.try_get("attempt").context("attempt should be present")?;
    let dispatched_at: DateTime<Utc> =
        row.try_get("dispatched_at").context("dispatched_at should be present")?;

    Ok(NodeAgentTask {
        task_id: row.try_get("task_id").context("task_id should be present")?,
        deployment_id: row.try_get("deployment_id").context("deployment_id should be present")?,
        target_id: row.try_get("target_id").context("target_id should be present")?,
        cluster_id: row.try_get("cluster_id").context("cluster_id should be present")?,
        node_id: row.try_get("node_id").context("node_id should be present")?,
        kind: kind
            .parse()
            .map_err(|error: String| anyhow!(error))
            .with_context(|| format!("invalid task kind `{kind}` loaded from postgres"))?,
        state: state
            .parse()
            .map_err(|error: String| anyhow!(error))
            .with_context(|| format!("invalid task state `{state}` loaded from postgres"))?,
        revision_id: row.try_get("revision_id").context("revision_id should be present")?,
        revision_version_label: row
            .try_get("revision_version_label")
            .context("revision_version_label should be present")?,
        source_path: row.try_get("source_path").context("source_path should be present")?,
        config_text: row.try_get("config_text").context("config_text should be present")?,
        attempt: u32::try_from(attempt).context("attempt should fit into u32")?,
        created_at_unix_ms: unix_time_ms(dispatched_at)?,
    })
}

fn count_to_u32(value: i64, field: &str) -> Result<u32> {
    u32::try_from(value).with_context(|| format!("{field} should fit into u32"))
}

fn unix_time_ms(value: DateTime<Utc>) -> Result<u64> {
    u64::try_from(value.timestamp_millis()).context("timestamp should fit into unix milliseconds")
}
