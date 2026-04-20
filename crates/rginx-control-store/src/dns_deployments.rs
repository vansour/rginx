use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use sqlx::{Postgres, Row, Transaction, postgres::PgRow};

use rginx_control_types::{
    DnsDeploymentDetail, DnsDeploymentStatus, DnsDeploymentSummary, DnsDeploymentTargetSummary,
    DnsRevisionDetail, DnsRevisionListItem, NodeLifecycleState,
};

use crate::repositories::{AuditLogListFilters, ControlPlaneStore, NewAuditLogEntry};

const DNS_DEPLOYMENT_EVENT_LIMIT: i64 = 20;

#[derive(Debug, Clone)]
pub struct CreateDnsDeploymentRecord {
    pub deployment_id: String,
    pub cluster_id: String,
    pub revision_id: String,
    pub created_by: String,
    pub parallelism: u32,
    pub failure_threshold: u32,
    pub auto_rollback: bool,
    pub promotes_cluster_runtime: bool,
    pub rollback_of_deployment_id: Option<String>,
    pub rollback_revision_id: Option<String>,
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct CreateDnsDeploymentTargetRecord {
    pub target_id: String,
    pub cluster_id: String,
    pub node_id: String,
    pub desired_revision_id: String,
    pub batch_index: u32,
}

#[derive(Debug, Clone)]
pub struct DnsDeploymentProgressSnapshot {
    pub deployment_id: String,
    pub cluster_id: String,
    pub revision_id: String,
    pub status: DnsDeploymentStatus,
    pub parallelism: u32,
    pub failure_threshold: u32,
    pub auto_rollback: bool,
    pub promotes_cluster_runtime: bool,
    pub rollback_of_deployment_id: Option<String>,
    pub rollback_revision_id: Option<String>,
    pub total_nodes: u32,
    pub pending_nodes: u32,
    pub active_nodes: u32,
    pub healthy_nodes: u32,
    pub failed_nodes: u32,
}

#[derive(Debug, Clone)]
pub struct ActiveDnsDeploymentTargetObservation {
    pub target_id: String,
    pub node_id: String,
    pub desired_revision_id: String,
    pub node_state: NodeLifecycleState,
    pub assigned_at: Option<DateTime<Utc>>,
    pub observed_revision_id: Option<String>,
    pub observed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NodeDnsOverride {
    pub node_id: String,
    pub published_revision_id: String,
}

#[derive(Debug, Clone)]
pub struct DnsDeploymentRepository {
    store: ControlPlaneStore,
}

impl DnsDeploymentRepository {
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
            from cp_dns_deployments
            where idempotency_key = $1
            "#,
        )
        .bind(idempotency_key)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| {
            format!("failed to load dns deployment by idempotency key `{idempotency_key}`")
        })
    }

    pub async fn find_active_cluster_deployment(
        &self,
        cluster_id: &str,
    ) -> Result<Option<DnsDeploymentSummary>> {
        sqlx::query(&dns_deployment_summary_select(
            "where d.cluster_id = $1 and d.status in ('running', 'paused') order by d.created_at asc limit 1",
        ))
        .bind(cluster_id)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| {
            format!("failed to load active dns deployment for cluster `{cluster_id}`")
        })?
        .map(map_dns_deployment_summary_row)
        .transpose()
    }

    pub async fn list_deployments(&self) -> Result<Vec<DnsDeploymentSummary>> {
        sqlx::query(&dns_deployment_summary_select(
            "order by d.created_at desc, d.deployment_id desc",
        ))
        .fetch_all(self.store.postgres())
        .await
        .context("failed to list dns deployments from postgres")?
        .into_iter()
        .map(map_dns_deployment_summary_row)
        .collect()
    }

    pub async fn load_deployment_detail(
        &self,
        deployment_id: &str,
    ) -> Result<Option<DnsDeploymentDetail>> {
        let Some(deployment) = self.load_deployment_summary(deployment_id).await? else {
            return Ok(None);
        };
        let revision =
            self.load_revision_summary(&deployment.revision_id).await?.ok_or_else(|| {
                anyhow!(
                    "dns revision `{}` for deployment `{deployment_id}` disappeared",
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
                t.batch_index,
                t.last_error,
                t.assigned_at,
                t.confirmed_at,
                t.failed_at
            from cp_dns_deployment_targets t
            join cp_nodes n on n.node_id = t.node_id
            where t.deployment_id = $1
            order by t.batch_index asc, t.node_id asc
            "#,
        )
        .bind(deployment_id)
        .fetch_all(self.store.postgres())
        .await
        .with_context(|| format!("failed to load dns deployment targets for `{deployment_id}`"))?
        .into_iter()
        .map(map_dns_deployment_target_row)
        .collect::<Result<Vec<_>>>()?;
        let recent_events = self
            .store
            .audit_repository()
            .list_summaries(&AuditLogListFilters {
                resource_type: Some("dns_deployment".to_string()),
                resource_id: Some(deployment_id.to_string()),
                limit: Some(DNS_DEPLOYMENT_EVENT_LIMIT),
                ..Default::default()
            })
            .await
            .with_context(|| {
                format!("failed to load dns deployment audit trail for `{deployment_id}`")
            })?;

        Ok(Some(DnsDeploymentDetail {
            deployment,
            revision,
            rollback_revision,
            targets,
            recent_events,
        }))
    }

    pub async fn create_deployment_with_audit(
        &self,
        record: &CreateDnsDeploymentRecord,
        targets: &[CreateDnsDeploymentTargetRecord],
        audit: &NewAuditLogEntry,
    ) -> Result<DnsDeploymentDetail> {
        let mut transaction = self
            .store
            .postgres()
            .begin()
            .await
            .context("failed to start dns deployment creation transaction")?;

        sqlx::query(
            r#"
            insert into cp_dns_deployments (
                deployment_id,
                cluster_id,
                revision_id,
                status,
                target_nodes,
                created_by,
                parallelism,
                failure_threshold,
                auto_rollback,
                promotes_cluster_runtime,
                rollback_of_deployment_id,
                rollback_revision_id,
                status_reason,
                idempotency_key,
                created_at,
                started_at,
                finished_at,
                updated_at
            )
            values (
                $1,
                $2,
                $3,
                'running',
                $4,
                $5,
                $6,
                $7,
                $8,
                $9,
                $10,
                $11,
                null,
                $12,
                $13,
                $13,
                null,
                $13
            )
            "#,
        )
        .bind(&record.deployment_id)
        .bind(&record.cluster_id)
        .bind(&record.revision_id)
        .bind(i32::try_from(targets.len()).context("dns target count should fit into i32")?)
        .bind(&record.created_by)
        .bind(i32::try_from(record.parallelism).context("parallelism should fit into i32")?)
        .bind(
            i32::try_from(record.failure_threshold)
                .context("failure_threshold should fit into i32")?,
        )
        .bind(record.auto_rollback)
        .bind(record.promotes_cluster_runtime)
        .bind(&record.rollback_of_deployment_id)
        .bind(&record.rollback_revision_id)
        .bind(&record.idempotency_key)
        .bind(record.created_at)
        .execute(&mut *transaction)
        .await
        .with_context(|| format!("failed to insert dns deployment `{}`", record.deployment_id))?;

        for target in targets {
            sqlx::query(
                r#"
                insert into cp_dns_deployment_targets (
                    target_id,
                    deployment_id,
                    cluster_id,
                    node_id,
                    desired_revision_id,
                    state,
                    batch_index,
                    last_error,
                    assigned_at,
                    confirmed_at,
                    failed_at,
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
                    "failed to insert dns deployment target `{}` for deployment `{}`",
                    target.target_id, record.deployment_id
                )
            })?;
        }

        insert_audit_entry(&mut transaction, audit).await?;
        transaction
            .commit()
            .await
            .context("failed to commit dns deployment creation transaction")?;

        self.load_deployment_detail(&record.deployment_id).await?.ok_or_else(|| {
            anyhow!("dns deployment `{}` disappeared after create commit", record.deployment_id)
        })
    }

    pub async fn list_active_deployment_ids(&self) -> Result<Vec<String>> {
        sqlx::query_scalar::<_, String>(
            r#"
            select deployment_id
            from cp_dns_deployments
            where status in ('running', 'paused')
            order by created_at asc, deployment_id asc
            "#,
        )
        .fetch_all(self.store.postgres())
        .await
        .context("failed to list active dns deployments from postgres")
    }

    pub async fn load_progress_snapshot(
        &self,
        deployment_id: &str,
    ) -> Result<Option<DnsDeploymentProgressSnapshot>> {
        sqlx::query(
            r#"
            select
                d.deployment_id,
                d.cluster_id,
                d.revision_id,
                d.status,
                d.parallelism,
                d.failure_threshold,
                d.auto_rollback,
                d.promotes_cluster_runtime,
                d.rollback_of_deployment_id,
                d.rollback_revision_id,
                count(t.target_id) as total_nodes,
                count(*) filter (where t.state = 'pending') as pending_nodes,
                count(*) filter (where t.state = 'active') as active_nodes,
                count(*) filter (where t.state = 'succeeded') as healthy_nodes,
                count(*) filter (where t.state = 'failed') as failed_nodes
            from cp_dns_deployments d
            left join cp_dns_deployment_targets t on t.deployment_id = d.deployment_id
            where d.deployment_id = $1
            group by
                d.deployment_id,
                d.cluster_id,
                d.revision_id,
                d.status,
                d.parallelism,
                d.failure_threshold,
                d.auto_rollback,
                d.promotes_cluster_runtime,
                d.rollback_of_deployment_id,
                d.rollback_revision_id
            "#,
        )
        .bind(deployment_id)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| format!("failed to load dns deployment progress for `{deployment_id}`"))?
        .map(map_dns_deployment_progress_row)
        .transpose()
    }

    pub async fn list_active_target_observations(
        &self,
        deployment_id: &str,
    ) -> Result<Vec<ActiveDnsDeploymentTargetObservation>> {
        sqlx::query(
            r#"
            select
                t.target_id,
                t.node_id,
                t.desired_revision_id,
                n.state as node_state,
                t.assigned_at,
                snap.observed_revision_id,
                snap.captured_at as observed_at
            from cp_dns_deployment_targets t
            join cp_nodes n on n.node_id = t.node_id
            left join lateral (
                select
                    captured_at,
                    status #>> '{dns,published_revision_id}' as observed_revision_id
                from cp_node_snapshots
                where node_id = t.node_id
                order by captured_at desc, snapshot_version desc
                limit 1
            ) snap on true
            where t.deployment_id = $1
              and t.state = 'active'
            order by t.batch_index asc, t.node_id asc
            "#,
        )
        .bind(deployment_id)
        .fetch_all(self.store.postgres())
        .await
        .with_context(|| {
            format!("failed to load active dns target observations for `{deployment_id}`")
        })?
        .into_iter()
        .map(map_active_dns_target_observation_row)
        .collect()
    }

    pub async fn activate_next_targets(&self, deployment_id: &str, limit: u32) -> Result<u32> {
        if limit == 0 {
            return Ok(0);
        }

        let mut transaction = self
            .store
            .postgres()
            .begin()
            .await
            .context("failed to start dns deployment activation transaction")?;
        let deployment = sqlx::query(
            r#"
            select cluster_id, revision_id
            from cp_dns_deployments
            where deployment_id = $1
              and status = 'running'
            for update
            "#,
        )
        .bind(deployment_id)
        .fetch_optional(&mut *transaction)
        .await
        .with_context(|| {
            format!("failed to lock dns deployment `{deployment_id}` for activation")
        })?;
        let Some(deployment) = deployment else {
            transaction
                .rollback()
                .await
                .context("failed to rollback missing dns deployment activation transaction")?;
            return Ok(0);
        };
        let cluster_id: String =
            deployment.try_get("cluster_id").context("cluster_id should be present")?;
        let revision_id: String =
            deployment.try_get("revision_id").context("revision_id should be present")?;
        let stable_revision_id = load_runtime_revision_id(&mut transaction, &cluster_id).await?;
        let limit = i64::from(limit.min(i32::MAX as u32));
        let selected = sqlx::query(
            r#"
            select target_id, node_id, desired_revision_id
            from cp_dns_deployment_targets
            where deployment_id = $1
              and state = 'pending'
            order by batch_index asc, node_id asc
            limit $2
            for update skip locked
            "#,
        )
        .bind(deployment_id)
        .bind(limit)
        .fetch_all(&mut *transaction)
        .await
        .with_context(|| {
            format!("failed to lock pending dns targets for deployment `{deployment_id}`")
        })?;
        if selected.is_empty() {
            transaction
                .commit()
                .await
                .context("failed to commit empty dns activation transaction")?;
            return Ok(0);
        }

        let now = Utc::now();
        let mut activated = 0_u32;
        for row in selected {
            let target_id: String =
                row.try_get("target_id").context("target_id should be present")?;
            let node_id: String = row.try_get("node_id").context("node_id should be present")?;
            let desired_revision_id: String = row
                .try_get("desired_revision_id")
                .context("desired_revision_id should be present")?;

            sqlx::query(
                r#"
                update cp_dns_deployment_targets
                set state = 'active',
                    assigned_at = $2,
                    updated_at = $2,
                    last_error = null,
                    failed_at = null
                where target_id = $1
                "#,
            )
            .bind(&target_id)
            .bind(now)
            .execute(&mut *transaction)
            .await
            .with_context(|| format!("failed to activate dns target `{target_id}`"))?;

            if stable_revision_id.as_deref() == Some(desired_revision_id.as_str()) {
                sqlx::query("delete from cp_dns_node_overrides where node_id = $1")
                    .bind(&node_id)
                    .execute(&mut *transaction)
                    .await
                    .with_context(|| {
                        format!("failed to clear dns override for node `{node_id}`")
                    })?;
            } else {
                sqlx::query(
                    r#"
                    insert into cp_dns_node_overrides (
                        node_id,
                        cluster_id,
                        published_revision_id,
                        deployment_id,
                        updated_at
                    )
                    values ($1, $2, $3, $4, $5)
                    on conflict (node_id)
                    do update set
                        cluster_id = excluded.cluster_id,
                        published_revision_id = excluded.published_revision_id,
                        deployment_id = excluded.deployment_id,
                        updated_at = excluded.updated_at
                    "#,
                )
                .bind(&node_id)
                .bind(&cluster_id)
                .bind(&revision_id)
                .bind(deployment_id)
                .bind(now)
                .execute(&mut *transaction)
                .await
                .with_context(|| format!("failed to upsert dns override for node `{node_id}`"))?;
            }
            activated = activated.saturating_add(1);
        }

        transaction
            .commit()
            .await
            .context("failed to commit dns deployment activation transaction")?;

        Ok(activated)
    }

    pub async fn mark_target_succeeded(&self, target_id: &str) -> Result<bool> {
        let result = sqlx::query(
            r#"
            update cp_dns_deployment_targets
            set state = 'succeeded',
                confirmed_at = now(),
                updated_at = now(),
                last_error = null
            where target_id = $1
              and state = 'active'
            "#,
        )
        .bind(target_id)
        .execute(self.store.postgres())
        .await
        .with_context(|| format!("failed to mark dns target `{target_id}` as succeeded"))?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn mark_target_failed(&self, target_id: &str, reason: &str) -> Result<bool> {
        let mut transaction = self
            .store
            .postgres()
            .begin()
            .await
            .context("failed to start dns target failure transaction")?;
        let row = sqlx::query(
            r#"
            select node_id
            from cp_dns_deployment_targets
            where target_id = $1
              and state = 'active'
            for update
            "#,
        )
        .bind(target_id)
        .fetch_optional(&mut *transaction)
        .await
        .with_context(|| format!("failed to lock dns target `{target_id}` for failure"))?;
        let Some(row) = row else {
            transaction
                .rollback()
                .await
                .context("failed to rollback missing dns target failure transaction")?;
            return Ok(false);
        };
        let node_id: String = row.try_get("node_id").context("node_id should be present")?;

        sqlx::query(
            r#"
            update cp_dns_deployment_targets
            set state = 'failed',
                last_error = $2,
                failed_at = now(),
                updated_at = now()
            where target_id = $1
            "#,
        )
        .bind(target_id)
        .bind(reason)
        .execute(&mut *transaction)
        .await
        .with_context(|| format!("failed to update dns target `{target_id}` failure state"))?;
        sqlx::query("delete from cp_dns_node_overrides where node_id = $1")
            .bind(&node_id)
            .execute(&mut *transaction)
            .await
            .with_context(|| format!("failed to clear dns override for node `{node_id}`"))?;

        transaction.commit().await.context("failed to commit dns target failure transaction")?;
        Ok(true)
    }

    pub async fn cancel_pending_targets(&self, deployment_id: &str, reason: &str) -> Result<u32> {
        let result = sqlx::query(
            r#"
            update cp_dns_deployment_targets
            set state = 'cancelled',
                last_error = $2,
                updated_at = now()
            where deployment_id = $1
              and state = 'pending'
            "#,
        )
        .bind(deployment_id)
        .bind(reason)
        .execute(self.store.postgres())
        .await
        .with_context(|| {
            format!("failed to cancel pending dns targets for deployment `{deployment_id}`")
        })?;
        Ok(result.rows_affected().min(u64::from(u32::MAX)) as u32)
    }

    pub async fn set_deployment_paused(
        &self,
        deployment_id: &str,
        paused: bool,
    ) -> Result<Option<DnsDeploymentSummary>> {
        let status = if paused { "paused" } else { "running" };
        sqlx::query(
            r#"
            update cp_dns_deployments
            set status = $2,
                updated_at = now()
            where deployment_id = $1
              and status in ('running', 'paused')
            "#,
        )
        .bind(deployment_id)
        .bind(status)
        .execute(self.store.postgres())
        .await
        .with_context(|| {
            format!("failed to set dns deployment `{deployment_id}` paused={paused}")
        })?;
        self.load_deployment_summary(deployment_id).await
    }

    pub async fn set_status_reason(&self, deployment_id: &str, reason: Option<&str>) -> Result<()> {
        sqlx::query(
            r#"
            update cp_dns_deployments
            set status_reason = $2,
                updated_at = now()
            where deployment_id = $1
            "#,
        )
        .bind(deployment_id)
        .bind(reason)
        .execute(self.store.postgres())
        .await
        .with_context(|| format!("failed to update dns deployment `{deployment_id}` reason"))?;
        Ok(())
    }

    pub async fn mark_deployment_succeeded(
        &self,
        deployment_id: &str,
        reason: Option<&str>,
    ) -> Result<Option<DnsDeploymentSummary>> {
        let mut transaction = self
            .store
            .postgres()
            .begin()
            .await
            .context("failed to start dns deployment success transaction")?;
        let row = sqlx::query(
            r#"
            select cluster_id, revision_id, promotes_cluster_runtime
            from cp_dns_deployments
            where deployment_id = $1
              and status in ('running', 'paused')
            for update
            "#,
        )
        .bind(deployment_id)
        .fetch_optional(&mut *transaction)
        .await
        .with_context(|| format!("failed to lock dns deployment `{deployment_id}` for success"))?;
        let Some(row) = row else {
            transaction
                .rollback()
                .await
                .context("failed to rollback missing dns deployment success transaction")?;
            return Ok(None);
        };
        let cluster_id: String =
            row.try_get("cluster_id").context("cluster_id should be present")?;
        let revision_id: String =
            row.try_get("revision_id").context("revision_id should be present")?;
        let promotes_cluster_runtime: bool = row
            .try_get("promotes_cluster_runtime")
            .context("promotes_cluster_runtime should be present")?;

        if promotes_cluster_runtime {
            sqlx::query(
                r#"
                insert into cp_dns_runtime_state (cluster_id, published_revision_id, updated_at)
                values ($1, $2, now())
                on conflict (cluster_id)
                do update set
                    published_revision_id = excluded.published_revision_id,
                    updated_at = excluded.updated_at
                "#,
            )
            .bind(&cluster_id)
            .bind(&revision_id)
            .execute(&mut *transaction)
            .await
            .with_context(|| {
                format!("failed to promote stable dns runtime for cluster `{cluster_id}`")
            })?;
            sqlx::query("delete from cp_dns_node_overrides where cluster_id = $1")
                .bind(&cluster_id)
                .execute(&mut *transaction)
                .await
                .with_context(|| {
                    format!("failed to clear dns overrides for cluster `{cluster_id}`")
                })?;
        }

        sqlx::query(
            r#"
            update cp_dns_deployments
            set status = 'succeeded',
                status_reason = $2,
                updated_at = now(),
                finished_at = now()
            where deployment_id = $1
            "#,
        )
        .bind(deployment_id)
        .bind(reason)
        .execute(&mut *transaction)
        .await
        .with_context(|| format!("failed to mark dns deployment `{deployment_id}` succeeded"))?;

        transaction
            .commit()
            .await
            .context("failed to commit dns deployment success transaction")?;
        self.load_deployment_summary(deployment_id).await
    }

    pub async fn mark_deployment_failed(
        &self,
        deployment_id: &str,
        reason: &str,
    ) -> Result<Option<DnsDeploymentSummary>> {
        sqlx::query(
            r#"
            update cp_dns_deployments
            set status = 'failed',
                status_reason = $2,
                updated_at = now(),
                finished_at = now()
            where deployment_id = $1
              and status in ('running', 'paused')
            "#,
        )
        .bind(deployment_id)
        .bind(reason)
        .execute(self.store.postgres())
        .await
        .with_context(|| format!("failed to mark dns deployment `{deployment_id}` failed"))?;
        self.load_deployment_summary(deployment_id).await
    }

    pub async fn mark_deployment_rolled_back(
        &self,
        deployment_id: &str,
        reason: &str,
    ) -> Result<Option<DnsDeploymentSummary>> {
        sqlx::query(
            r#"
            update cp_dns_deployments
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
        .with_context(|| {
            format!("failed to mark dns deployment `{deployment_id}` as rolled back")
        })?;
        self.load_deployment_summary(deployment_id).await
    }

    pub async fn find_rollback_child(
        &self,
        deployment_id: &str,
    ) -> Result<Option<DnsDeploymentSummary>> {
        sqlx::query(&dns_deployment_summary_select(
            "where d.rollback_of_deployment_id = $1 order by d.created_at desc limit 1",
        ))
        .bind(deployment_id)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| {
            format!("failed to load dns rollback child for deployment `{deployment_id}`")
        })?
        .map(map_dns_deployment_summary_row)
        .transpose()
    }

    pub async fn list_succeeded_target_node_ids(&self, deployment_id: &str) -> Result<Vec<String>> {
        sqlx::query_scalar::<_, String>(
            r#"
            select node_id
            from cp_dns_deployment_targets
            where deployment_id = $1
              and state = 'succeeded'
            order by batch_index asc, node_id asc
            "#,
        )
        .bind(deployment_id)
        .fetch_all(self.store.postgres())
        .await
        .with_context(|| format!("failed to load succeeded dns target nodes for `{deployment_id}`"))
    }

    pub async fn load_runtime_revision_for_cluster(
        &self,
        cluster_id: &str,
    ) -> Result<Option<DnsRevisionListItem>> {
        sqlx::query(
            r#"
            select
                r.revision_id,
                r.cluster_id,
                r.version_label,
                r.summary,
                r.created_by,
                r.created_at,
                r.published_at
            from cp_dns_runtime_state s
            join cp_dns_revisions r on r.revision_id = s.published_revision_id
            where s.cluster_id = $1
            "#,
        )
        .bind(cluster_id)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| {
            format!("failed to load stable dns runtime revision for cluster `{cluster_id}`")
        })?
        .map(map_dns_revision_list_row)
        .transpose()
    }

    pub async fn load_effective_revision_for_node(
        &self,
        cluster_id: &str,
        node_id: &str,
    ) -> Result<Option<DnsRevisionDetail>> {
        sqlx::query(
            r#"
            select
                r.revision_id,
                r.cluster_id,
                r.version_label,
                r.summary,
                r.plan_json,
                r.validation_summary,
                r.created_by,
                r.created_at,
                r.published_at
            from cp_clusters c
            left join cp_dns_node_overrides o
                on o.cluster_id = c.cluster_id
               and o.node_id = $2
            left join cp_dns_runtime_state s
                on s.cluster_id = c.cluster_id
            join cp_dns_revisions r
                on r.revision_id = coalesce(o.published_revision_id, s.published_revision_id)
            where c.cluster_id = $1
            "#,
        )
        .bind(cluster_id)
        .bind(node_id)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| {
            format!(
                "failed to load effective dns revision for cluster `{cluster_id}` node `{node_id}`"
            )
        })?
        .map(map_dns_revision_detail_row)
        .transpose()
    }

    pub async fn list_cluster_node_overrides(
        &self,
        cluster_id: &str,
    ) -> Result<Vec<NodeDnsOverride>> {
        sqlx::query(
            r#"
            select node_id, published_revision_id
            from cp_dns_node_overrides
            where cluster_id = $1
            order by node_id asc
            "#,
        )
        .bind(cluster_id)
        .fetch_all(self.store.postgres())
        .await
        .with_context(|| format!("failed to list dns node overrides for cluster `{cluster_id}`"))?
        .into_iter()
        .map(|row| {
            Ok(NodeDnsOverride {
                node_id: row.try_get("node_id").context("node_id should be present")?,
                published_revision_id: row
                    .try_get("published_revision_id")
                    .context("published_revision_id should be present")?,
            })
        })
        .collect()
    }

    async fn load_deployment_summary(
        &self,
        deployment_id: &str,
    ) -> Result<Option<DnsDeploymentSummary>> {
        sqlx::query(&dns_deployment_summary_select("where d.deployment_id = $1"))
            .bind(deployment_id)
            .fetch_optional(self.store.postgres())
            .await
            .with_context(|| {
                format!("failed to load dns deployment summary for `{deployment_id}`")
            })?
            .map(map_dns_deployment_summary_row)
            .transpose()
    }

    async fn load_revision_summary(
        &self,
        revision_id: &str,
    ) -> Result<Option<DnsRevisionListItem>> {
        sqlx::query(
            r#"
            select
                revision_id,
                cluster_id,
                version_label,
                summary,
                created_by,
                created_at,
                published_at
            from cp_dns_revisions
            where revision_id = $1
            "#,
        )
        .bind(revision_id)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| format!("failed to load dns revision summary `{revision_id}`"))?
        .map(map_dns_revision_list_row)
        .transpose()
    }
}

async fn load_runtime_revision_id(
    transaction: &mut Transaction<'_, Postgres>,
    cluster_id: &str,
) -> Result<Option<String>> {
    sqlx::query_scalar::<_, String>(
        r#"
        select published_revision_id
        from cp_dns_runtime_state
        where cluster_id = $1
        "#,
    )
    .bind(cluster_id)
    .fetch_optional(&mut **transaction)
    .await
    .with_context(|| format!("failed to load stable dns runtime id for cluster `{cluster_id}`"))
}

async fn insert_audit_entry(
    transaction: &mut Transaction<'_, Postgres>,
    audit: &NewAuditLogEntry,
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
    .bind(&audit.audit_id)
    .bind(&audit.request_id)
    .bind(&audit.cluster_id)
    .bind(&audit.actor_id)
    .bind(&audit.action)
    .bind(&audit.resource_type)
    .bind(&audit.resource_id)
    .bind(&audit.result)
    .bind(&audit.details)
    .bind(audit.created_at)
    .execute(&mut **transaction)
    .await
    .with_context(|| format!("failed to insert audit log `{}`", audit.audit_id))?;
    Ok(())
}

fn dns_deployment_summary_select(suffix: &str) -> String {
    format!(
        r#"
        with target_stats as (
            select
                deployment_id,
                count(*)::bigint as target_nodes,
                count(*) filter (where state = 'succeeded')::bigint as healthy_nodes,
                count(*) filter (where state = 'failed')::bigint as failed_nodes,
                count(*) filter (where state = 'active')::bigint as active_nodes,
                count(*) filter (where state = 'pending')::bigint as pending_nodes
            from cp_dns_deployment_targets
            group by deployment_id
        )
        select
            d.deployment_id,
            d.cluster_id,
            d.revision_id,
            r.version_label as revision_version_label,
            d.status,
            coalesce(ts.target_nodes, d.target_nodes::bigint) as target_nodes,
            coalesce(ts.healthy_nodes, 0::bigint) as healthy_nodes,
            coalesce(ts.failed_nodes, 0::bigint) as failed_nodes,
            coalesce(ts.active_nodes, 0::bigint) as active_nodes,
            coalesce(ts.pending_nodes, 0::bigint) as pending_nodes,
            d.parallelism,
            d.failure_threshold,
            d.auto_rollback,
            d.promotes_cluster_runtime,
            d.created_by,
            d.rollback_of_deployment_id,
            d.rollback_revision_id,
            rb.deployment_id as rolled_back_by_deployment_id,
            d.status_reason,
            d.created_at,
            d.started_at,
            d.finished_at
        from cp_dns_deployments d
        join cp_dns_revisions r on r.revision_id = d.revision_id
        left join target_stats ts on ts.deployment_id = d.deployment_id
        left join cp_dns_deployments rb on rb.rollback_of_deployment_id = d.deployment_id
        {suffix}
        "#
    )
}

fn map_dns_deployment_summary_row(row: PgRow) -> Result<DnsDeploymentSummary> {
    let status = row.try_get::<String, _>("status").context("status should be present")?;
    Ok(DnsDeploymentSummary {
        deployment_id: row.try_get("deployment_id").context("deployment_id should be present")?,
        cluster_id: row.try_get("cluster_id").context("cluster_id should be present")?,
        revision_id: row.try_get("revision_id").context("revision_id should be present")?,
        revision_version_label: row
            .try_get("revision_version_label")
            .context("revision_version_label should be present")?,
        status: status
            .parse()
            .map_err(|error: String| anyhow!(error))
            .with_context(|| format!("failed to parse dns deployment status `{status}`"))?,
        target_nodes: count_to_u32(row.try_get("target_nodes")?, "target_nodes")?,
        healthy_nodes: count_to_u32(row.try_get("healthy_nodes")?, "healthy_nodes")?,
        failed_nodes: count_to_u32(row.try_get("failed_nodes")?, "failed_nodes")?,
        active_nodes: count_to_u32(row.try_get("active_nodes")?, "active_nodes")?,
        pending_nodes: count_to_u32(row.try_get("pending_nodes")?, "pending_nodes")?,
        parallelism: u32::try_from(
            row.try_get::<i32, _>("parallelism").context("parallelism should be present")?,
        )
        .context("parallelism should fit into u32")?,
        failure_threshold: u32::try_from(
            row.try_get::<i32, _>("failure_threshold")
                .context("failure_threshold should be present")?,
        )
        .context("failure_threshold should fit into u32")?,
        auto_rollback: row.try_get("auto_rollback").context("auto_rollback should be present")?,
        promotes_cluster_runtime: row
            .try_get("promotes_cluster_runtime")
            .context("promotes_cluster_runtime should be present")?,
        created_by: row.try_get("created_by").context("created_by should be present")?,
        rollback_of_deployment_id: row
            .try_get("rollback_of_deployment_id")
            .context("rollback_of_deployment_id should be readable")?,
        rollback_revision_id: row
            .try_get("rollback_revision_id")
            .context("rollback_revision_id should be readable")?,
        rolled_back_by_deployment_id: row
            .try_get("rolled_back_by_deployment_id")
            .context("rolled_back_by_deployment_id should be readable")?,
        status_reason: row.try_get("status_reason").context("status_reason should be readable")?,
        created_at_unix_ms: unix_time_ms(
            row.try_get::<DateTime<Utc>, _>("created_at")
                .context("created_at should be present")?,
        )?,
        started_at_unix_ms: row
            .try_get::<Option<DateTime<Utc>>, _>("started_at")
            .context("started_at should be readable")?
            .map(unix_time_ms)
            .transpose()?,
        finished_at_unix_ms: row
            .try_get::<Option<DateTime<Utc>>, _>("finished_at")
            .context("finished_at should be readable")?
            .map(unix_time_ms)
            .transpose()?,
    })
}

fn map_dns_deployment_target_row(row: PgRow) -> Result<DnsDeploymentTargetSummary> {
    let state = row.try_get::<String, _>("state").context("state should be present")?;
    let node_state =
        row.try_get::<String, _>("node_state").context("node_state should be present")?;

    Ok(DnsDeploymentTargetSummary {
        target_id: row.try_get("target_id").context("target_id should be present")?,
        deployment_id: row.try_get("deployment_id").context("deployment_id should be present")?,
        node_id: row.try_get("node_id").context("node_id should be present")?,
        advertise_addr: row
            .try_get("advertise_addr")
            .context("advertise_addr should be present")?,
        node_state: node_state
            .parse()
            .map_err(|error: String| anyhow!(error))
            .with_context(|| format!("failed to parse node state `{node_state}`"))?,
        desired_revision_id: row
            .try_get("desired_revision_id")
            .context("desired_revision_id should be present")?,
        state: state
            .parse()
            .map_err(|error: String| anyhow!(error))
            .with_context(|| format!("failed to parse dns target state `{state}`"))?,
        batch_index: u32::try_from(
            row.try_get::<i32, _>("batch_index").context("batch_index should be present")?,
        )
        .context("batch_index should fit into u32")?,
        last_error: row.try_get("last_error").context("last_error should be readable")?,
        assigned_at_unix_ms: row
            .try_get::<Option<DateTime<Utc>>, _>("assigned_at")
            .context("assigned_at should be readable")?
            .map(unix_time_ms)
            .transpose()?,
        confirmed_at_unix_ms: row
            .try_get::<Option<DateTime<Utc>>, _>("confirmed_at")
            .context("confirmed_at should be readable")?
            .map(unix_time_ms)
            .transpose()?,
        failed_at_unix_ms: row
            .try_get::<Option<DateTime<Utc>>, _>("failed_at")
            .context("failed_at should be readable")?
            .map(unix_time_ms)
            .transpose()?,
    })
}

fn map_dns_deployment_progress_row(row: PgRow) -> Result<DnsDeploymentProgressSnapshot> {
    let status = row.try_get::<String, _>("status").context("status should be present")?;
    Ok(DnsDeploymentProgressSnapshot {
        deployment_id: row.try_get("deployment_id").context("deployment_id should be present")?,
        cluster_id: row.try_get("cluster_id").context("cluster_id should be present")?,
        revision_id: row.try_get("revision_id").context("revision_id should be present")?,
        status: status
            .parse()
            .map_err(|error: String| anyhow!(error))
            .with_context(|| format!("failed to parse dns deployment status `{status}`"))?,
        parallelism: u32::try_from(
            row.try_get::<i32, _>("parallelism").context("parallelism should be present")?,
        )
        .context("parallelism should fit into u32")?,
        failure_threshold: u32::try_from(
            row.try_get::<i32, _>("failure_threshold")
                .context("failure_threshold should be present")?,
        )
        .context("failure_threshold should fit into u32")?,
        auto_rollback: row.try_get("auto_rollback").context("auto_rollback should be present")?,
        promotes_cluster_runtime: row
            .try_get("promotes_cluster_runtime")
            .context("promotes_cluster_runtime should be present")?,
        rollback_of_deployment_id: row
            .try_get("rollback_of_deployment_id")
            .context("rollback_of_deployment_id should be readable")?,
        rollback_revision_id: row
            .try_get("rollback_revision_id")
            .context("rollback_revision_id should be readable")?,
        total_nodes: count_to_u32(row.try_get("total_nodes")?, "total_nodes")?,
        pending_nodes: count_to_u32(row.try_get("pending_nodes")?, "pending_nodes")?,
        active_nodes: count_to_u32(row.try_get("active_nodes")?, "active_nodes")?,
        healthy_nodes: count_to_u32(row.try_get("healthy_nodes")?, "healthy_nodes")?,
        failed_nodes: count_to_u32(row.try_get("failed_nodes")?, "failed_nodes")?,
    })
}

fn map_active_dns_target_observation_row(
    row: PgRow,
) -> Result<ActiveDnsDeploymentTargetObservation> {
    let node_state =
        row.try_get::<String, _>("node_state").context("node_state should be present")?;
    Ok(ActiveDnsDeploymentTargetObservation {
        target_id: row.try_get("target_id").context("target_id should be present")?,
        node_id: row.try_get("node_id").context("node_id should be present")?,
        desired_revision_id: row
            .try_get("desired_revision_id")
            .context("desired_revision_id should be present")?,
        node_state: node_state
            .parse()
            .map_err(|error: String| anyhow!(error))
            .with_context(|| format!("failed to parse node state `{node_state}`"))?,
        assigned_at: row.try_get("assigned_at").context("assigned_at should be readable")?,
        observed_revision_id: row
            .try_get("observed_revision_id")
            .context("observed_revision_id should be readable")?,
        observed_at: row.try_get("observed_at").context("observed_at should be readable")?,
    })
}

fn map_dns_revision_list_row(row: PgRow) -> Result<DnsRevisionListItem> {
    Ok(DnsRevisionListItem {
        revision_id: row.try_get("revision_id").context("revision_id should be present")?,
        cluster_id: row.try_get("cluster_id").context("cluster_id should be present")?,
        version_label: row.try_get("version_label").context("version_label should be present")?,
        summary: row.try_get("summary").context("summary should be present")?,
        created_by: row.try_get("created_by").context("created_by should be present")?,
        created_at_unix_ms: unix_time_ms(
            row.try_get::<DateTime<Utc>, _>("created_at")
                .context("created_at should be present")?,
        )?,
        published_at_unix_ms: row
            .try_get::<Option<DateTime<Utc>>, _>("published_at")
            .context("published_at should be readable")?
            .map(unix_time_ms)
            .transpose()?,
    })
}

fn map_dns_revision_detail_row(row: PgRow) -> Result<DnsRevisionDetail> {
    let plan =
        row.try_get::<serde_json::Value, _>("plan_json").context("plan_json should be readable")?;
    let validation = row
        .try_get::<serde_json::Value, _>("validation_summary")
        .context("validation_summary should be readable")?;

    Ok(DnsRevisionDetail {
        revision_id: row.try_get("revision_id").context("revision_id should be present")?,
        cluster_id: row.try_get("cluster_id").context("cluster_id should be present")?,
        version_label: row.try_get("version_label").context("version_label should be present")?,
        summary: row.try_get("summary").context("summary should be present")?,
        plan: serde_json::from_value(plan).context("failed to decode dns plan_json")?,
        validation: serde_json::from_value(validation)
            .context("failed to decode dns validation_summary")?,
        created_by: row.try_get("created_by").context("created_by should be present")?,
        created_at_unix_ms: unix_time_ms(
            row.try_get::<DateTime<Utc>, _>("created_at")
                .context("created_at should be present")?,
        )?,
        published_at_unix_ms: row
            .try_get::<Option<DateTime<Utc>>, _>("published_at")
            .context("published_at should be readable")?
            .map(unix_time_ms)
            .transpose()?,
    })
}

fn count_to_u32(value: i64, field: &str) -> Result<u32> {
    u32::try_from(value).with_context(|| format!("{field} should fit into u32"))
}

fn unix_time_ms(value: DateTime<Utc>) -> Result<u64> {
    let millis = value.timestamp_millis();
    u64::try_from(millis).context("timestamp should not be negative")
}
