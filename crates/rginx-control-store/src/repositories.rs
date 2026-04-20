use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgPoolOptions};

use rginx_control_types::{
    AuditLogEntry, AuditLogSummary, AuthRole, AuthSessionSummary, AuthUserSummary,
    AuthenticatedActor, ConfigCompileSummary, ConfigDraftDetail, ConfigDraftSummary,
    ConfigDraftValidationState, ConfigRevisionDetail, ConfigRevisionListItem,
    ConfigRevisionSummary, ConfigValidationReport, DeploymentSummary, DnsDeploymentSummary,
    NodeAgentRegistrationRequest, NodeSnapshotDetail, NodeSnapshotIngestRequest, NodeSnapshotMeta,
    NodeSummary,
};

use crate::config::ControlPlaneStoreConfig;
use crate::dragonfly::DragonflyKeyspace;

const DASHBOARD_RECENT_LIMIT: i64 = 10;
const RECENT_AUDIT_LIMIT: i64 = 10;
const RECENT_NODE_SNAPSHOT_LIMIT: i64 = 12;

#[derive(Debug, Clone)]
pub struct DashboardSnapshot {
    pub total_clusters: u32,
    pub total_nodes: u32,
    pub online_nodes: u32,
    pub draining_nodes: u32,
    pub offline_nodes: u32,
    pub drifted_nodes: u32,
    pub total_revisions: u64,
    pub active_deployments: u32,
    pub active_dns_deployments: u32,
    pub latest_revision: Option<ConfigRevisionSummary>,
    pub recent_nodes: Vec<NodeSummary>,
    pub recent_deployments: Vec<DeploymentSummary>,
    pub recent_dns_deployments: Vec<DnsDeploymentSummary>,
}

#[derive(Debug, Clone)]
pub struct BackendDependencyStatus {
    pub name: String,
    pub endpoint: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct WorkerRuntimeContext {
    pub known_nodes: usize,
    pub active_deployments: usize,
    pub active_dns_deployments: usize,
    pub dependencies: Vec<BackendDependencyStatus>,
}

#[derive(Debug, Clone)]
pub struct StoredPasswordUser {
    pub user: AuthUserSummary,
    pub password_hash: String,
}

#[derive(Debug, Clone)]
pub struct NewAuthSession {
    pub session_id: String,
    pub user_id: String,
    pub session_hash: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub user_agent: Option<String>,
    pub remote_addr: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewAuditLogEntry {
    pub audit_id: String,
    pub request_id: String,
    pub cluster_id: Option<String>,
    pub actor_id: String,
    pub action: String,
    pub resource_type: String,
    pub resource_id: String,
    pub result: String,
    pub details: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
pub struct AuditLogListFilters {
    pub cluster_id: Option<String>,
    pub actor_id: Option<String>,
    pub action: Option<String>,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub result: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct NewConfigDraftRecord {
    pub draft_id: String,
    pub cluster_id: String,
    pub title: String,
    pub summary: String,
    pub source_path: String,
    pub config_text: String,
    pub base_revision_id: Option<String>,
    pub created_by: String,
    pub updated_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct UpdateConfigDraftRecord {
    pub title: String,
    pub summary: String,
    pub source_path: String,
    pub config_text: String,
    pub base_revision_id: Option<String>,
    pub updated_by: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct DraftValidationRecord {
    pub validation_state: ConfigDraftValidationState,
    pub validation_errors: Vec<String>,
    pub compile_summary: Option<ConfigCompileSummary>,
    pub validated_at: Option<DateTime<Utc>>,
    pub updated_by: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewConfigRevisionRecord {
    pub revision_id: String,
    pub cluster_id: String,
    pub version_label: String,
    pub summary: String,
    pub source_path: String,
    pub config_text: String,
    pub compile_summary: Option<ConfigCompileSummary>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ControlPlaneStore {
    config: Arc<ControlPlaneStoreConfig>,
    postgres: PgPool,
    dragonfly: Arc<DragonflyKeyspace>,
}

impl ControlPlaneStore {
    pub fn new(config: ControlPlaneStoreConfig) -> Self {
        let postgres = PgPoolOptions::new()
            .max_connections(config.db_max_connections)
            .connect_lazy_with(config.pg_connect_options());
        let dragonfly = DragonflyKeyspace::new(config.dragonfly_key_prefix.clone());

        Self { config: Arc::new(config), postgres, dragonfly: Arc::new(dragonfly) }
    }

    pub fn config(&self) -> &ControlPlaneStoreConfig {
        self.config.as_ref()
    }

    pub fn postgres(&self) -> &PgPool {
        &self.postgres
    }

    pub fn dragonfly_keyspace(&self) -> &DragonflyKeyspace {
        self.dragonfly.as_ref()
    }

    pub fn dashboard_repository(&self) -> DashboardRepository {
        DashboardRepository { store: self.clone() }
    }

    pub fn dependency_repository(&self) -> DependencyRepository {
        DependencyRepository { store: self.clone() }
    }

    pub fn revision_repository(&self) -> RevisionRepository {
        RevisionRepository { store: self.clone() }
    }

    pub fn deployment_repository(&self) -> crate::DeploymentRepository {
        crate::DeploymentRepository::new(self.clone())
    }

    pub fn dns_repository(&self) -> crate::DnsRepository {
        crate::DnsRepository::new(self.clone())
    }

    pub fn dns_deployment_repository(&self) -> crate::DnsDeploymentRepository {
        crate::DnsDeploymentRepository::new(self.clone())
    }

    pub fn worker_runtime_repository(&self) -> WorkerRuntimeRepository {
        WorkerRuntimeRepository { store: self.clone() }
    }

    pub fn audit_repository(&self) -> AuditRepository {
        AuditRepository { store: self.clone() }
    }

    pub fn auth_repository(&self) -> AuthRepository {
        AuthRepository { store: self.clone() }
    }

    pub fn node_repository(&self) -> NodeRepository {
        NodeRepository { store: self.clone() }
    }
}

#[derive(Debug, Clone)]
pub struct DashboardRepository {
    store: ControlPlaneStore,
}

impl DashboardRepository {
    pub async fn load_snapshot(&self) -> Result<DashboardSnapshot> {
        let aggregate_row = sqlx::query(
            r#"
            select
                (select count(*) from cp_clusters) as total_clusters,
                (select count(*) from cp_nodes) as total_nodes,
                (select count(*) from cp_nodes where state = 'online') as online_nodes,
                (select count(*) from cp_nodes where state = 'draining') as draining_nodes,
                (select count(*) from cp_nodes where state = 'offline') as offline_nodes,
                (select count(*) from cp_nodes where state = 'drifted') as drifted_nodes,
                (select count(*) from cp_config_revisions) as total_revisions,
                (select count(*) from cp_deployments where status in ('running', 'paused')) as active_deployments,
                (select count(*) from cp_dns_deployments where status in ('running', 'paused')) as active_dns_deployments
            "#,
        )
        .fetch_one(self.store.postgres())
        .await
        .context("failed to load dashboard aggregates from postgres")?;

        let latest_revision = sqlx::query(
            r#"
            select revision_id, cluster_id, version_label, summary, created_at
            from cp_config_revisions
            order by created_at desc
            limit 1
            "#,
        )
        .fetch_optional(self.store.postgres())
        .await
        .context("failed to load latest revision from postgres")?
        .map(map_revision_row)
        .transpose()?;

        let recent_nodes = sqlx::query(
            r#"
            select
                node_id,
                cluster_id,
                advertise_addr,
                role,
                state,
                running_version,
                admin_socket_path,
                last_snapshot_version,
                runtime_revision,
                runtime_pid,
                listener_count,
                active_connections,
                status_reason,
                coalesce(last_seen_at, created_at) as observed_at
            from cp_nodes
            order by coalesce(last_seen_at, created_at) desc, created_at desc
            limit $1
            "#,
        )
        .bind(DASHBOARD_RECENT_LIMIT)
        .fetch_all(self.store.postgres())
        .await
        .context("failed to load recent nodes from postgres")?
        .into_iter()
        .map(map_node_row)
        .collect::<Result<Vec<_>>>()?;

        let recent_deployments = sqlx::query(
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
            order by d.created_at desc
            limit $1
            "#,
        )
        .bind(DASHBOARD_RECENT_LIMIT)
        .fetch_all(self.store.postgres())
        .await
        .context("failed to load recent deployments from postgres")?
        .into_iter()
        .map(map_deployment_row)
        .collect::<Result<Vec<_>>>()?;
        let recent_dns_deployments = self
            .store
            .dns_deployment_repository()
            .list_deployments()
            .await?
            .into_iter()
            .take(usize::try_from(DASHBOARD_RECENT_LIMIT).unwrap_or_default())
            .collect::<Vec<_>>();

        Ok(DashboardSnapshot {
            total_clusters: count_to_u32(
                aggregate_row.try_get("total_clusters")?,
                "total_clusters",
            )?,
            total_nodes: count_to_u32(aggregate_row.try_get("total_nodes")?, "total_nodes")?,
            online_nodes: count_to_u32(aggregate_row.try_get("online_nodes")?, "online_nodes")?,
            draining_nodes: count_to_u32(
                aggregate_row.try_get("draining_nodes")?,
                "draining_nodes",
            )?,
            offline_nodes: count_to_u32(aggregate_row.try_get("offline_nodes")?, "offline_nodes")?,
            drifted_nodes: count_to_u32(aggregate_row.try_get("drifted_nodes")?, "drifted_nodes")?,
            total_revisions: count_to_u64(
                aggregate_row.try_get("total_revisions")?,
                "total_revisions",
            )?,
            active_deployments: count_to_u32(
                aggregate_row.try_get("active_deployments")?,
                "active_deployments",
            )?,
            active_dns_deployments: count_to_u32(
                aggregate_row.try_get("active_dns_deployments")?,
                "active_dns_deployments",
            )?,
            latest_revision,
            recent_nodes,
            recent_deployments,
            recent_dns_deployments,
        })
    }
}

#[derive(Debug, Clone)]
pub struct RevisionRepository {
    store: ControlPlaneStore,
}

impl RevisionRepository {
    pub async fn cluster_exists(&self, cluster_id: &str) -> Result<bool> {
        let count =
            sqlx::query_scalar::<_, i64>("select count(*) from cp_clusters where cluster_id = $1")
                .bind(cluster_id)
                .fetch_one(self.store.postgres())
                .await
                .with_context(|| format!("failed to check cluster `{cluster_id}` existence"))?;
        Ok(count > 0)
    }

    pub async fn list_revisions(&self) -> Result<Vec<ConfigRevisionListItem>> {
        sqlx::query(
            r#"
            select
                revision_id,
                cluster_id,
                version_label,
                summary,
                created_by,
                created_at
            from cp_config_revisions
            order by created_at desc, revision_id desc
            "#,
        )
        .fetch_all(self.store.postgres())
        .await
        .context("failed to list config revisions from postgres")?
        .into_iter()
        .map(map_revision_list_item_row)
        .collect()
    }

    pub async fn load_revision_detail(
        &self,
        revision_id: &str,
    ) -> Result<Option<ConfigRevisionDetail>> {
        sqlx::query(
            r#"
            select
                revision_id,
                cluster_id,
                version_label,
                summary,
                created_by,
                created_at,
                source_path,
                config_text,
                compile_summary
            from cp_config_revisions
            where revision_id = $1
            "#,
        )
        .bind(revision_id)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| format!("failed to load config revision `{revision_id}` from postgres"))?
        .map(map_revision_detail_row)
        .transpose()
    }

    pub async fn load_latest_revision_for_cluster(
        &self,
        cluster_id: &str,
    ) -> Result<Option<ConfigRevisionListItem>> {
        sqlx::query(
            r#"
            select
                revision_id,
                cluster_id,
                version_label,
                summary,
                created_by,
                created_at
            from cp_config_revisions
            where cluster_id = $1
            order by created_at desc, revision_id desc
            limit 1
            "#,
        )
        .bind(cluster_id)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| {
            format!("failed to load latest config revision for cluster `{cluster_id}`")
        })?
        .map(map_revision_list_item_row)
        .transpose()
    }

    pub async fn list_drafts(&self) -> Result<Vec<ConfigDraftSummary>> {
        sqlx::query(
            r#"
            select
                draft_id,
                cluster_id,
                title,
                summary,
                base_revision_id,
                validation_state,
                published_revision_id,
                created_by,
                updated_by,
                created_at,
                updated_at
            from cp_config_drafts
            order by updated_at desc, draft_id desc
            "#,
        )
        .fetch_all(self.store.postgres())
        .await
        .context("failed to list config drafts from postgres")?
        .into_iter()
        .map(map_draft_summary_row)
        .collect()
    }

    pub async fn load_draft_detail(&self, draft_id: &str) -> Result<Option<ConfigDraftDetail>> {
        sqlx::query(
            r#"
            select
                draft_id,
                cluster_id,
                title,
                summary,
                source_path,
                config_text,
                base_revision_id,
                validation_state,
                validation_errors,
                compile_summary,
                validated_at,
                published_revision_id,
                created_by,
                updated_by,
                created_at,
                updated_at
            from cp_config_drafts
            where draft_id = $1
            "#,
        )
        .bind(draft_id)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| format!("failed to load config draft `{draft_id}` from postgres"))?
        .map(map_draft_detail_row)
        .transpose()
    }

    pub async fn create_draft_with_audit(
        &self,
        draft: &NewConfigDraftRecord,
        audit: &NewAuditLogEntry,
    ) -> Result<ConfigDraftDetail> {
        let mut transaction = self
            .store
            .postgres()
            .begin()
            .await
            .context("failed to start config draft transaction")?;

        sqlx::query(
            r#"
            insert into cp_config_drafts (
                draft_id,
                cluster_id,
                title,
                summary,
                source_path,
                config_text,
                base_revision_id,
                validation_state,
                validation_errors,
                compile_summary,
                validated_at,
                published_revision_id,
                created_by,
                updated_by,
                created_at,
                updated_at
            )
            values (
                $1, $2, $3, $4, $5, $6, $7, 'pending', '[]'::jsonb, '{}'::jsonb, null, null, $8, $9, $10, $11
            )
            "#,
        )
        .bind(&draft.draft_id)
        .bind(&draft.cluster_id)
        .bind(&draft.title)
        .bind(&draft.summary)
        .bind(&draft.source_path)
        .bind(&draft.config_text)
        .bind(&draft.base_revision_id)
        .bind(&draft.created_by)
        .bind(&draft.updated_by)
        .bind(draft.created_at)
        .bind(draft.updated_at)
        .execute(&mut *transaction)
        .await
        .with_context(|| format!("failed to insert config draft `{}`", draft.draft_id))?;

        insert_audit_entry(&mut transaction, audit).await?;
        transaction.commit().await.context("failed to commit config draft transaction")?;

        self.load_draft_detail(&draft.draft_id).await?.ok_or_else(|| {
            anyhow::anyhow!("config draft `{}` disappeared after insert commit", draft.draft_id)
        })
    }

    pub async fn update_draft_with_audit(
        &self,
        draft_id: &str,
        update: &UpdateConfigDraftRecord,
        audit: &NewAuditLogEntry,
    ) -> Result<Option<ConfigDraftDetail>> {
        let mut transaction = self
            .store
            .postgres()
            .begin()
            .await
            .context("failed to start config draft transaction")?;

        let result = sqlx::query(
            r#"
            update cp_config_drafts
            set
                title = $2,
                summary = $3,
                source_path = $4,
                config_text = $5,
                base_revision_id = $6,
                validation_state = 'pending',
                validation_errors = '[]'::jsonb,
                compile_summary = '{}'::jsonb,
                validated_at = null,
                published_revision_id = null,
                updated_by = $7,
                updated_at = $8
            where draft_id = $1
            "#,
        )
        .bind(draft_id)
        .bind(&update.title)
        .bind(&update.summary)
        .bind(&update.source_path)
        .bind(&update.config_text)
        .bind(&update.base_revision_id)
        .bind(&update.updated_by)
        .bind(update.updated_at)
        .execute(&mut *transaction)
        .await
        .with_context(|| format!("failed to update config draft `{draft_id}`"))?;

        if result.rows_affected() == 0 {
            transaction
                .rollback()
                .await
                .context("failed to rollback missing config draft update")?;
            return Ok(None);
        }

        insert_audit_entry(&mut transaction, audit).await?;
        transaction.commit().await.context("failed to commit config draft update transaction")?;

        self.load_draft_detail(draft_id).await
    }

    pub async fn store_validation_with_audit(
        &self,
        draft_id: &str,
        validation: &DraftValidationRecord,
        audit: &NewAuditLogEntry,
    ) -> Result<Option<ConfigDraftDetail>> {
        let mut transaction = self
            .store
            .postgres()
            .begin()
            .await
            .context("failed to start draft validation transaction")?;
        let validation_errors = serde_json::to_value(&validation.validation_errors)
            .context("failed to encode draft validation errors")?;
        let compile_summary = compile_summary_value(validation.compile_summary.as_ref())?;

        let result = sqlx::query(
            r#"
            update cp_config_drafts
            set
                validation_state = $2,
                validation_errors = $3,
                compile_summary = $4,
                validated_at = $5,
                updated_by = $6,
                updated_at = $7
            where draft_id = $1
            "#,
        )
        .bind(draft_id)
        .bind(validation.validation_state.as_str())
        .bind(validation_errors)
        .bind(compile_summary)
        .bind(validation.validated_at)
        .bind(&validation.updated_by)
        .bind(validation.updated_at)
        .execute(&mut *transaction)
        .await
        .with_context(|| format!("failed to persist validation report for draft `{draft_id}`"))?;

        if result.rows_affected() == 0 {
            transaction
                .rollback()
                .await
                .context("failed to rollback missing draft validation update")?;
            return Ok(None);
        }

        insert_audit_entry(&mut transaction, audit).await?;
        transaction.commit().await.context("failed to commit draft validation transaction")?;

        self.load_draft_detail(draft_id).await
    }

    pub async fn publish_draft_with_audit(
        &self,
        draft_id: &str,
        revision: &NewConfigRevisionRecord,
        audit: &NewAuditLogEntry,
    ) -> Result<Option<(ConfigDraftDetail, ConfigRevisionDetail)>> {
        let mut transaction = self
            .store
            .postgres()
            .begin()
            .await
            .context("failed to start revision publish transaction")?;
        let compile_summary = compile_summary_value(revision.compile_summary.as_ref())?;
        let rendered_config = serde_json::json!({
            "source_path": revision.source_path,
            "compile_summary": compile_summary.clone(),
        });

        sqlx::query(
            r#"
            insert into cp_config_revisions (
                revision_id,
                cluster_id,
                version_label,
                summary,
                rendered_config,
                created_at,
                created_by,
                source_path,
                config_text,
                compile_summary
            )
            values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(&revision.revision_id)
        .bind(&revision.cluster_id)
        .bind(&revision.version_label)
        .bind(&revision.summary)
        .bind(rendered_config)
        .bind(revision.created_at)
        .bind(&revision.created_by)
        .bind(&revision.source_path)
        .bind(&revision.config_text)
        .bind(compile_summary.clone())
        .execute(&mut *transaction)
        .await
        .with_context(|| {
            format!("failed to insert published revision `{}`", revision.revision_id)
        })?;

        let result = sqlx::query(
            r#"
            update cp_config_drafts
            set
                validation_state = 'published',
                validation_errors = '[]'::jsonb,
                compile_summary = $3,
                validated_at = $4,
                published_revision_id = $2,
                updated_by = $5,
                updated_at = $4
            where draft_id = $1
            "#,
        )
        .bind(draft_id)
        .bind(&revision.revision_id)
        .bind(compile_summary)
        .bind(revision.created_at)
        .bind(&revision.created_by)
        .execute(&mut *transaction)
        .await
        .with_context(|| format!("failed to mark draft `{draft_id}` as published"))?;

        if result.rows_affected() == 0 {
            transaction
                .rollback()
                .await
                .context("failed to rollback missing draft publish update")?;
            return Ok(None);
        }

        insert_audit_entry(&mut transaction, audit).await?;
        transaction.commit().await.context("failed to commit revision publish transaction")?;

        let draft = self.load_draft_detail(draft_id).await?.ok_or_else(|| {
            anyhow::anyhow!("draft `{draft_id}` disappeared after publish commit")
        })?;
        let revision =
            self.load_revision_detail(&revision.revision_id).await?.ok_or_else(|| {
                anyhow::anyhow!(
                    "revision `{}` disappeared after publish commit",
                    revision.revision_id
                )
            })?;
        Ok(Some((draft, revision)))
    }
}

#[derive(Debug, Clone)]
pub struct DependencyRepository {
    store: ControlPlaneStore,
}

impl DependencyRepository {
    pub async fn ensure_postgres_ready(&self) -> Result<()> {
        sqlx::query("select 1")
            .execute(self.store.postgres())
            .await
            .context("failed to reach postgres for control-plane store readiness check")?;
        Ok(())
    }

    pub async fn load_dependency_statuses(&self) -> Vec<BackendDependencyStatus> {
        let postgres_status =
            if self.ensure_postgres_ready().await.is_ok() { "ready" } else { "unreachable" };

        vec![
            BackendDependencyStatus {
                name: "postgres".to_string(),
                endpoint: self.store.config().postgres_endpoint(),
                status: postgres_status.to_string(),
            },
            BackendDependencyStatus {
                name: "dragonfly".to_string(),
                endpoint: self.store.config().dragonfly_endpoint(),
                status: "keyspace-planned".to_string(),
            },
        ]
    }
}

#[derive(Debug, Clone)]
pub struct WorkerRuntimeRepository {
    store: ControlPlaneStore,
}

impl WorkerRuntimeRepository {
    pub async fn load_runtime_context(&self) -> Result<WorkerRuntimeContext> {
        let known_nodes = sqlx::query_scalar::<_, i64>("select count(*) from cp_nodes")
            .fetch_one(self.store.postgres())
            .await
            .context("failed to count control-plane nodes")?;
        let active_deployments = sqlx::query_scalar::<_, i64>(
            "select count(*) from cp_deployments where status in ('running', 'paused')",
        )
        .fetch_one(self.store.postgres())
        .await
        .context("failed to count active control-plane deployments")?;
        let active_dns_deployments = sqlx::query_scalar::<_, i64>(
            "select count(*) from cp_dns_deployments where status in ('running', 'paused')",
        )
        .fetch_one(self.store.postgres())
        .await
        .context("failed to count active dns deployments")?;
        let dependencies = self.store.dependency_repository().load_dependency_statuses().await;

        Ok(WorkerRuntimeContext {
            known_nodes: count_to_usize(known_nodes, "known_nodes")?,
            active_deployments: count_to_usize(active_deployments, "active_deployments")?,
            active_dns_deployments: count_to_usize(
                active_dns_deployments,
                "active_dns_deployments",
            )?,
            dependencies,
        })
    }
}

#[derive(Debug, Clone)]
pub struct NodeRepository {
    store: ControlPlaneStore,
}

impl NodeRepository {
    pub async fn list_nodes(&self) -> Result<Vec<NodeSummary>> {
        sqlx::query(
            r#"
            select
                node_id,
                cluster_id,
                advertise_addr,
                role,
                state,
                running_version,
                admin_socket_path,
                last_snapshot_version,
                runtime_revision,
                runtime_pid,
                listener_count,
                active_connections,
                status_reason,
                coalesce(last_seen_at, created_at) as observed_at
            from cp_nodes
            order by
                case state
                    when 'drifted' then 0
                    when 'offline' then 1
                    when 'draining' then 2
                    when 'online' then 3
                    else 4
                end asc,
                coalesce(last_seen_at, created_at) desc,
                node_id asc
            "#,
        )
        .fetch_all(self.store.postgres())
        .await
        .context("failed to list control-plane nodes from postgres")?
        .into_iter()
        .map(map_node_row)
        .collect()
    }

    pub async fn load_node_summary(&self, node_id: &str) -> Result<Option<NodeSummary>> {
        sqlx::query(
            r#"
            select
                node_id,
                cluster_id,
                advertise_addr,
                role,
                state,
                running_version,
                admin_socket_path,
                last_snapshot_version,
                runtime_revision,
                runtime_pid,
                listener_count,
                active_connections,
                status_reason,
                coalesce(last_seen_at, created_at) as observed_at
            from cp_nodes
            where node_id = $1
            "#,
        )
        .bind(node_id)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| format!("failed to load control-plane node `{node_id}`"))?
        .map(map_node_row)
        .transpose()
    }

    pub async fn find_stale_nodes(
        &self,
        observed_before: DateTime<Utc>,
    ) -> Result<Vec<NodeSummary>> {
        sqlx::query(
            r#"
            select
                node_id,
                cluster_id,
                advertise_addr,
                role,
                state,
                running_version,
                admin_socket_path,
                last_snapshot_version,
                runtime_revision,
                runtime_pid,
                listener_count,
                active_connections,
                status_reason,
                coalesce(last_seen_at, created_at) as observed_at
            from cp_nodes
            where last_seen_at is not null
              and last_seen_at < $1
              and state <> 'offline'
            order by last_seen_at asc, node_id asc
            "#,
        )
        .bind(observed_before)
        .fetch_all(self.store.postgres())
        .await
        .context("failed to load stale control-plane nodes from postgres")?
        .into_iter()
        .map(map_node_row)
        .collect()
    }

    pub async fn upsert_report_with_audit(
        &self,
        report: &NodeAgentRegistrationRequest,
        audit: &NewAuditLogEntry,
    ) -> Result<NodeSummary> {
        let observed_at = utc_from_unix_ms(report.observed_at_unix_ms)?;
        let mut transaction = self
            .store
            .postgres()
            .begin()
            .await
            .context("failed to start node register/heartbeat transaction")?;

        sqlx::query(
            r#"
            insert into cp_nodes (
                node_id,
                cluster_id,
                advertise_addr,
                role,
                state,
                running_version,
                admin_socket_path,
                last_seen_at,
                last_snapshot_version,
                runtime_revision,
                runtime_pid,
                listener_count,
                active_connections,
                status_reason,
                created_at,
                updated_at
            )
            values (
                $1,
                $2,
                $3,
                $4,
                $5,
                $6,
                $7,
                $8,
                $9,
                $10,
                $11,
                $12,
                $13,
                $14,
                now(),
                now()
            )
            on conflict (node_id) do update
            set cluster_id = excluded.cluster_id,
                advertise_addr = excluded.advertise_addr,
                role = excluded.role,
                state = excluded.state,
                running_version = excluded.running_version,
                admin_socket_path = excluded.admin_socket_path,
                last_seen_at = excluded.last_seen_at,
                last_snapshot_version = excluded.last_snapshot_version,
                runtime_revision = excluded.runtime_revision,
                runtime_pid = excluded.runtime_pid,
                listener_count = excluded.listener_count,
                active_connections = excluded.active_connections,
                status_reason = excluded.status_reason,
                updated_at = now()
            "#,
        )
        .bind(&report.node_id)
        .bind(&report.cluster_id)
        .bind(&report.advertise_addr)
        .bind(&report.role)
        .bind(report.state.as_str())
        .bind(&report.running_version)
        .bind(&report.admin_socket_path)
        .bind(observed_at)
        .bind(option_u64_to_i64(report.runtime.snapshot_version, "snapshot_version")?)
        .bind(option_u64_to_i64(report.runtime.revision, "revision")?)
        .bind(option_u32_to_i32(report.runtime.pid, "pid")?)
        .bind(option_u32_to_i32(report.runtime.listener_count, "listener_count")?)
        .bind(option_u32_to_i32(report.runtime.active_connections, "active_connections")?)
        .bind(&report.runtime.error)
        .execute(&mut *transaction)
        .await
        .with_context(|| format!("failed to upsert control-plane node `{}`", report.node_id))?;

        sqlx::query(
            r#"
            insert into cp_node_heartbeats (
                node_id,
                admin_socket_path,
                state,
                observed_at,
                payload
            )
            values ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(&report.node_id)
        .bind(&report.admin_socket_path)
        .bind(report.state.as_str())
        .bind(observed_at)
        .bind(heartbeat_payload(report))
        .execute(&mut *transaction)
        .await
        .with_context(|| format!("failed to insert node heartbeat for `{}`", report.node_id))?;

        insert_audit_entry(&mut transaction, audit).await?;
        transaction
            .commit()
            .await
            .context("failed to commit node register/heartbeat transaction")?;

        self.load_node_summary(&report.node_id).await?.ok_or_else(|| {
            anyhow::anyhow!(
                "node `{}` should exist after register/heartbeat upsert",
                report.node_id
            )
        })
    }

    pub async fn mark_node_offline_with_audit(
        &self,
        node_id: &str,
        observed_before: DateTime<Utc>,
        reason: &str,
        audit: &NewAuditLogEntry,
    ) -> Result<Option<NodeSummary>> {
        let mut transaction = self
            .store
            .postgres()
            .begin()
            .await
            .context("failed to start node offline reconciliation transaction")?;

        let rows_affected = sqlx::query(
            r#"
            update cp_nodes
            set state = 'offline',
                status_reason = $3,
                updated_at = now()
            where node_id = $1
              and last_seen_at is not null
              and last_seen_at < $2
              and state <> 'offline'
            "#,
        )
        .bind(node_id)
        .bind(observed_before)
        .bind(reason)
        .execute(&mut *transaction)
        .await
        .with_context(|| format!("failed to reconcile node `{node_id}` to offline"))?
        .rows_affected();

        if rows_affected == 0 {
            transaction
                .rollback()
                .await
                .context("failed to rollback no-op node reconciliation transaction")?;
            return Ok(None);
        }

        insert_audit_entry(&mut transaction, audit).await?;
        transaction
            .commit()
            .await
            .context("failed to commit node offline reconciliation transaction")?;

        self.load_node_summary(node_id).await
    }

    pub async fn upsert_snapshot_with_audit(
        &self,
        snapshot: &NodeSnapshotIngestRequest,
        audit: &NewAuditLogEntry,
    ) -> Result<NodeSnapshotMeta> {
        let captured_at = utc_from_unix_ms(snapshot.captured_at_unix_ms)?;
        let mut transaction = self
            .store
            .postgres()
            .begin()
            .await
            .context("failed to start node snapshot ingest transaction")?;

        sqlx::query(
            r#"
            insert into cp_node_snapshots (
                node_id,
                snapshot_version,
                schema_version,
                captured_at,
                pid,
                binary_version,
                included_modules,
                status,
                counters,
                traffic,
                peer_health,
                upstreams,
                payload
            )
            values (
                $1,
                $2,
                $3,
                $4,
                $5,
                $6,
                $7,
                $8,
                $9,
                $10,
                $11,
                $12,
                $13
            )
            on conflict (node_id, snapshot_version) do update
            set schema_version = excluded.schema_version,
                captured_at = excluded.captured_at,
                pid = excluded.pid,
                binary_version = excluded.binary_version,
                included_modules = excluded.included_modules,
                status = excluded.status,
                counters = excluded.counters,
                traffic = excluded.traffic,
                peer_health = excluded.peer_health,
                upstreams = excluded.upstreams,
                payload = excluded.payload,
                created_at = now()
            "#,
        )
        .bind(&snapshot.node_id)
        .bind(
            i64::try_from(snapshot.snapshot_version)
                .context("snapshot_version should fit into i64")?,
        )
        .bind(i32::try_from(snapshot.schema_version).context("schema_version should fit into i32")?)
        .bind(captured_at)
        .bind(i32::try_from(snapshot.pid).context("pid should fit into i32")?)
        .bind(&snapshot.binary_version)
        .bind(serde_json::json!(snapshot.included_modules))
        .bind(&snapshot.status)
        .bind(&snapshot.counters)
        .bind(&snapshot.traffic)
        .bind(&snapshot.peer_health)
        .bind(&snapshot.upstreams)
        .bind(snapshot_payload(snapshot))
        .execute(&mut *transaction)
        .await
        .with_context(|| {
            format!(
                "failed to upsert snapshot version `{}` for node `{}`",
                snapshot.snapshot_version, snapshot.node_id
            )
        })?;

        insert_audit_entry(&mut transaction, audit).await?;
        transaction.commit().await.context("failed to commit node snapshot ingest transaction")?;

        self.load_latest_snapshot_meta(&snapshot.node_id).await?.ok_or_else(|| {
            anyhow::anyhow!(
                "snapshot version `{}` for node `{}` should exist after ingest",
                snapshot.snapshot_version,
                snapshot.node_id
            )
        })
    }

    pub async fn load_latest_snapshot_detail(
        &self,
        node_id: &str,
    ) -> Result<Option<NodeSnapshotDetail>> {
        sqlx::query(
            r#"
            select
                node_id,
                snapshot_version,
                schema_version,
                captured_at,
                pid,
                binary_version,
                included_modules,
                status,
                counters,
                traffic,
                peer_health,
                upstreams
            from cp_node_snapshots
            where node_id = $1
            order by captured_at desc, snapshot_version desc
            limit 1
            "#,
        )
        .bind(node_id)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| format!("failed to load latest snapshot detail for node `{node_id}`"))?
        .map(map_node_snapshot_detail_row)
        .transpose()
    }

    pub async fn list_recent_snapshot_metas(&self, node_id: &str) -> Result<Vec<NodeSnapshotMeta>> {
        sqlx::query(
            r#"
            select
                node_id,
                snapshot_version,
                schema_version,
                captured_at,
                pid,
                binary_version,
                included_modules
            from cp_node_snapshots
            where node_id = $1
            order by captured_at desc, snapshot_version desc
            limit $2
            "#,
        )
        .bind(node_id)
        .bind(RECENT_NODE_SNAPSHOT_LIMIT)
        .fetch_all(self.store.postgres())
        .await
        .with_context(|| format!("failed to list recent snapshots for node `{node_id}`"))?
        .into_iter()
        .map(map_node_snapshot_meta_row)
        .collect()
    }

    pub async fn count_snapshots(&self) -> Result<u64> {
        let count = sqlx::query_scalar::<_, i64>("select count(*) from cp_node_snapshots")
            .fetch_one(self.store.postgres())
            .await
            .context("failed to count node snapshots")?;
        count_to_u64(count, "node_snapshots_total")
    }

    async fn load_latest_snapshot_meta(&self, node_id: &str) -> Result<Option<NodeSnapshotMeta>> {
        sqlx::query(
            r#"
            select
                node_id,
                snapshot_version,
                schema_version,
                captured_at,
                pid,
                binary_version,
                included_modules
            from cp_node_snapshots
            where node_id = $1
            order by captured_at desc, snapshot_version desc
            limit 1
            "#,
        )
        .bind(node_id)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| format!("failed to load latest snapshot meta for node `{node_id}`"))?
        .map(map_node_snapshot_meta_row)
        .transpose()
    }
}

#[derive(Debug, Clone)]
pub struct AuditRepository {
    store: ControlPlaneStore,
}

impl AuditRepository {
    pub async fn list_recent(&self) -> Result<Vec<AuditLogSummary>> {
        self.list_summaries(&AuditLogListFilters::default()).await
    }

    pub async fn list_summaries(
        &self,
        filters: &AuditLogListFilters,
    ) -> Result<Vec<AuditLogSummary>> {
        sqlx::query(
            r#"
            select
                audit_id,
                request_id,
                cluster_id,
                actor_id,
                action,
                resource_type,
                resource_id,
                result,
                created_at
            from cp_audit_logs
            where ($1::text is null or cluster_id = $1)
              and ($2::text is null or actor_id = $2)
              and ($3::text is null or action = $3)
              and ($4::text is null or resource_type = $4)
              and ($5::text is null or resource_id = $5)
              and ($6::text is null or result = $6)
            order by created_at desc, audit_id desc
            limit $7
            "#,
        )
        .bind(&filters.cluster_id)
        .bind(&filters.actor_id)
        .bind(&filters.action)
        .bind(&filters.resource_type)
        .bind(&filters.resource_id)
        .bind(&filters.result)
        .bind(filters.limit.unwrap_or(RECENT_AUDIT_LIMIT))
        .fetch_all(self.store.postgres())
        .await
        .context("failed to load audit log summaries from postgres")?
        .into_iter()
        .map(map_audit_row)
        .collect()
    }

    pub async fn list_entries(&self, filters: &AuditLogListFilters) -> Result<Vec<AuditLogEntry>> {
        sqlx::query(
            r#"
            select
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
            from cp_audit_logs
            where ($1::text is null or cluster_id = $1)
              and ($2::text is null or actor_id = $2)
              and ($3::text is null or action = $3)
              and ($4::text is null or resource_type = $4)
              and ($5::text is null or resource_id = $5)
              and ($6::text is null or result = $6)
            order by created_at desc, audit_id desc
            limit $7
            "#,
        )
        .bind(&filters.cluster_id)
        .bind(&filters.actor_id)
        .bind(&filters.action)
        .bind(&filters.resource_type)
        .bind(&filters.resource_id)
        .bind(&filters.result)
        .bind(filters.limit.unwrap_or(RECENT_AUDIT_LIMIT))
        .fetch_all(self.store.postgres())
        .await
        .context("failed to load audit log entries from postgres")?
        .into_iter()
        .map(map_audit_entry_row)
        .collect()
    }

    pub async fn list_recent_for_resource(
        &self,
        resource_type: &str,
        resource_id: &str,
        limit: i64,
    ) -> Result<Vec<AuditLogSummary>> {
        sqlx::query(
            r#"
            select
                audit_id,
                request_id,
                cluster_id,
                actor_id,
                action,
                resource_type,
                resource_id,
                result,
                created_at
            from cp_audit_logs
            where resource_type = $1
              and resource_id = $2
            order by created_at desc
            limit $3
            "#,
        )
        .bind(resource_type)
        .bind(resource_id)
        .bind(limit)
        .fetch_all(self.store.postgres())
        .await
        .with_context(|| {
            format!("failed to load recent audit logs for resource `{resource_type}/{resource_id}`")
        })?
        .into_iter()
        .map(map_audit_row)
        .collect()
    }

    pub async fn list_timeline_for_deployment(
        &self,
        deployment_id: &str,
        limit: i64,
    ) -> Result<Vec<AuditLogSummary>> {
        sqlx::query(
            r#"
            select
                audit_id,
                request_id,
                cluster_id,
                actor_id,
                action,
                resource_type,
                resource_id,
                result,
                created_at
            from cp_audit_logs
            where (resource_type = 'deployment' and resource_id = $1)
               or (resource_type = 'deployment_task' and details ->> 'deployment_id' = $1)
            order by created_at desc, audit_id desc
            limit $2
            "#,
        )
        .bind(deployment_id)
        .bind(limit)
        .fetch_all(self.store.postgres())
        .await
        .with_context(|| format!("failed to load deployment timeline for `{deployment_id}`"))?
        .into_iter()
        .map(map_audit_row)
        .collect()
    }

    pub async fn load_entry(&self, audit_id: &str) -> Result<Option<AuditLogEntry>> {
        sqlx::query(
            r#"
            select
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
            from cp_audit_logs
            where audit_id = $1
            "#,
        )
        .bind(audit_id)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| format!("failed to load audit log `{audit_id}`"))?
        .map(map_audit_entry_row)
        .transpose()
    }

    pub async fn count_all(&self) -> Result<u64> {
        let count = sqlx::query_scalar::<_, i64>("select count(*) from cp_audit_logs")
            .fetch_one(self.store.postgres())
            .await
            .context("failed to count audit logs")?;
        count_to_u64(count, "audit_logs_total")
    }

    pub async fn insert_entry(&self, entry: &NewAuditLogEntry) -> Result<()> {
        let mut transaction =
            self.store.postgres().begin().await.context("failed to start audit log transaction")?;
        insert_audit_entry(&mut transaction, entry).await?;
        transaction.commit().await.context("failed to commit audit log transaction")?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct AuthRepository {
    store: ControlPlaneStore,
}

impl AuthRepository {
    pub async fn find_user_credentials_by_username(
        &self,
        username: &str,
    ) -> Result<Option<StoredPasswordUser>> {
        let row = sqlx::query(
            r#"
            select user_id, username, display_name, password_hash, is_active, created_at
            from cp_users
            where username = $1
            "#,
        )
        .bind(username)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| format!("failed to load control-plane user `{username}`"))?;

        match row {
            Some(row) => {
                let user_id: String =
                    row.try_get("user_id").context("user_id should be present")?;
                let roles = load_roles_for_user(self.store.postgres(), &user_id).await?;
                Ok(Some(StoredPasswordUser {
                    password_hash: row
                        .try_get("password_hash")
                        .context("password_hash should be present")?,
                    user: AuthUserSummary {
                        user_id,
                        username: row.try_get("username").context("username should be present")?,
                        display_name: row
                            .try_get("display_name")
                            .context("display_name should be present")?,
                        active: row.try_get("is_active").context("is_active should be present")?,
                        roles,
                        created_at_unix_ms: unix_time_ms(
                            row.try_get::<DateTime<Utc>, _>("created_at")
                                .context("created_at should be present")?,
                        )?,
                    },
                }))
            }
            None => Ok(None),
        }
    }

    pub async fn load_actor_by_session_hash(
        &self,
        session_hash: &str,
    ) -> Result<Option<AuthenticatedActor>> {
        let row = sqlx::query(
            r#"
            select
                s.session_id,
                s.user_id,
                s.issued_at,
                s.expires_at,
                u.username,
                u.display_name,
                u.is_active,
                u.created_at
            from cp_api_sessions s
            join cp_users u on u.user_id = s.user_id
            where s.session_hash = $1
              and s.revoked_at is null
              and s.expires_at > now()
              and u.is_active = true
            "#,
        )
        .bind(session_hash)
        .fetch_optional(self.store.postgres())
        .await
        .context("failed to load authenticated control-plane session")?;

        match row {
            Some(row) => {
                let user_id: String =
                    row.try_get("user_id").context("user_id should be present")?;
                let session_id: String =
                    row.try_get("session_id").context("session_id should be present")?;
                let roles = load_roles_for_user(self.store.postgres(), &user_id).await?;

                sqlx::query(
                    r#"
                    update cp_api_sessions
                    set last_seen_at = now()
                    where session_id = $1
                    "#,
                )
                .bind(&session_id)
                .execute(self.store.postgres())
                .await
                .with_context(|| {
                    format!("failed to update last_seen_at for session `{session_id}`")
                })?;

                Ok(Some(AuthenticatedActor {
                    user: AuthUserSummary {
                        user_id,
                        username: row.try_get("username").context("username should be present")?,
                        display_name: row
                            .try_get("display_name")
                            .context("display_name should be present")?,
                        active: row.try_get("is_active").context("is_active should be present")?,
                        roles,
                        created_at_unix_ms: unix_time_ms(
                            row.try_get::<DateTime<Utc>, _>("created_at")
                                .context("created_at should be present")?,
                        )?,
                    },
                    session: AuthSessionSummary {
                        session_id,
                        issued_at_unix_ms: unix_time_ms(
                            row.try_get::<DateTime<Utc>, _>("issued_at")
                                .context("issued_at should be present")?,
                        )?,
                        expires_at_unix_ms: unix_time_ms(
                            row.try_get::<DateTime<Utc>, _>("expires_at")
                                .context("expires_at should be present")?,
                        )?,
                    },
                }))
            }
            None => Ok(None),
        }
    }

    pub async fn create_session_with_audit(
        &self,
        session: &NewAuthSession,
        audit: &NewAuditLogEntry,
    ) -> Result<AuthSessionSummary> {
        let mut transaction = self
            .store
            .postgres()
            .begin()
            .await
            .context("failed to start session creation transaction")?;

        sqlx::query(
            r#"
            insert into cp_api_sessions (
                session_id,
                user_id,
                session_hash,
                issued_at,
                expires_at,
                last_seen_at,
                user_agent,
                remote_addr
            )
            values ($1, $2, $3, $4, $5, $4, $6, $7)
            "#,
        )
        .bind(&session.session_id)
        .bind(&session.user_id)
        .bind(&session.session_hash)
        .bind(session.issued_at)
        .bind(session.expires_at)
        .bind(&session.user_agent)
        .bind(&session.remote_addr)
        .execute(&mut *transaction)
        .await
        .with_context(|| {
            format!("failed to create control-plane session `{}`", session.session_id)
        })?;

        insert_audit_entry(&mut transaction, audit).await?;

        transaction.commit().await.context("failed to commit session creation transaction")?;

        Ok(AuthSessionSummary {
            session_id: session.session_id.clone(),
            issued_at_unix_ms: unix_time_ms(session.issued_at)?,
            expires_at_unix_ms: unix_time_ms(session.expires_at)?,
        })
    }

    pub async fn revoke_session_with_audit(
        &self,
        session_id: &str,
        audit: &NewAuditLogEntry,
    ) -> Result<bool> {
        let mut transaction = self
            .store
            .postgres()
            .begin()
            .await
            .context("failed to start session revoke transaction")?;

        let rows_affected = sqlx::query(
            r#"
            update cp_api_sessions
            set revoked_at = now(),
                last_seen_at = now()
            where session_id = $1
              and revoked_at is null
            "#,
        )
        .bind(session_id)
        .execute(&mut *transaction)
        .await
        .with_context(|| format!("failed to revoke control-plane session `{session_id}`"))?
        .rows_affected();

        insert_audit_entry(&mut transaction, audit).await?;

        transaction.commit().await.context("failed to commit session revoke transaction")?;

        Ok(rows_affected > 0)
    }
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

async fn load_roles_for_user(pool: &PgPool, user_id: &str) -> Result<Vec<AuthRole>> {
    let rows = sqlx::query(
        r#"
        select role_id
        from cp_user_roles
        where user_id = $1
        order by role_id asc
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .with_context(|| format!("failed to load roles for user `{user_id}`"))?;

    let mut roles = rows
        .into_iter()
        .map(|row| {
            let role = row.try_get::<String, _>("role_id").context("role_id should be present")?;
            role.parse()
                .map_err(|error: String| anyhow::anyhow!(error))
                .with_context(|| format!("invalid role `{role}` loaded from postgres"))
        })
        .collect::<Result<Vec<_>>>()?;
    roles.sort();
    roles.dedup();
    Ok(roles)
}

fn map_revision_row(row: sqlx::postgres::PgRow) -> Result<ConfigRevisionSummary> {
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

fn map_revision_list_item_row(row: sqlx::postgres::PgRow) -> Result<ConfigRevisionListItem> {
    Ok(ConfigRevisionListItem {
        revision_id: row.try_get("revision_id").context("revision_id should be present")?,
        cluster_id: row.try_get("cluster_id").context("cluster_id should be present")?,
        version_label: row.try_get("version_label").context("version_label should be present")?,
        summary: row.try_get("summary").context("summary should be present")?,
        created_by: row.try_get("created_by").context("created_by should be present")?,
        created_at_unix_ms: unix_time_ms(
            row.try_get::<DateTime<Utc>, _>("created_at")
                .context("created_at should be present")?,
        )?,
    })
}

fn map_revision_detail_row(row: sqlx::postgres::PgRow) -> Result<ConfigRevisionDetail> {
    let compile_summary: Value =
        row.try_get("compile_summary").context("compile_summary should be readable")?;

    Ok(ConfigRevisionDetail {
        revision_id: row.try_get("revision_id").context("revision_id should be present")?,
        cluster_id: row.try_get("cluster_id").context("cluster_id should be present")?,
        version_label: row.try_get("version_label").context("version_label should be present")?,
        summary: row.try_get("summary").context("summary should be present")?,
        created_by: row.try_get("created_by").context("created_by should be present")?,
        created_at_unix_ms: unix_time_ms(
            row.try_get::<DateTime<Utc>, _>("created_at")
                .context("created_at should be present")?,
        )?,
        source_path: row.try_get("source_path").context("source_path should be present")?,
        config_text: row.try_get("config_text").context("config_text should be present")?,
        compile_summary: parse_compile_summary_value(compile_summary)?,
    })
}

fn map_draft_summary_row(row: sqlx::postgres::PgRow) -> Result<ConfigDraftSummary> {
    let validation_state = row
        .try_get::<String, _>("validation_state")
        .context("validation_state should be present")?;

    Ok(ConfigDraftSummary {
        draft_id: row.try_get("draft_id").context("draft_id should be present")?,
        cluster_id: row.try_get("cluster_id").context("cluster_id should be present")?,
        title: row.try_get("title").context("title should be present")?,
        summary: row.try_get("summary").context("summary should be present")?,
        base_revision_id: row
            .try_get("base_revision_id")
            .context("base_revision_id should be readable")?,
        validation_state: validation_state
            .parse()
            .map_err(|error: String| anyhow::anyhow!(error))
            .with_context(|| {
                format!("invalid validation_state `{validation_state}` loaded from postgres")
            })?,
        published_revision_id: row
            .try_get("published_revision_id")
            .context("published_revision_id should be readable")?,
        created_by: row.try_get("created_by").context("created_by should be present")?,
        updated_by: row.try_get("updated_by").context("updated_by should be present")?,
        created_at_unix_ms: unix_time_ms(
            row.try_get::<DateTime<Utc>, _>("created_at")
                .context("created_at should be present")?,
        )?,
        updated_at_unix_ms: unix_time_ms(
            row.try_get::<DateTime<Utc>, _>("updated_at")
                .context("updated_at should be present")?,
        )?,
    })
}

fn map_draft_detail_row(row: sqlx::postgres::PgRow) -> Result<ConfigDraftDetail> {
    let validation_state = row
        .try_get::<String, _>("validation_state")
        .context("validation_state should be present")?;
    let validation_errors: Value =
        row.try_get("validation_errors").context("validation_errors should be readable")?;
    let compile_summary: Value =
        row.try_get("compile_summary").context("compile_summary should be readable")?;
    let validated_at: Option<DateTime<Utc>> =
        row.try_get("validated_at").context("validated_at should be readable")?;
    let issues = json_array_to_strings(validation_errors, "validation_errors")?;
    let summary = parse_compile_summary_value(compile_summary)?;
    let state =
        validation_state.parse().map_err(|error: String| anyhow::anyhow!(error)).with_context(
            || format!("invalid validation_state `{validation_state}` loaded from postgres"),
        )?;
    let last_validation = if validated_at.is_some() || !issues.is_empty() || summary.is_some() {
        Some(ConfigValidationReport {
            valid: matches!(
                state,
                ConfigDraftValidationState::Valid | ConfigDraftValidationState::Published
            ),
            validated_at_unix_ms: validated_at.map(unix_time_ms).transpose()?.unwrap_or_default(),
            normalized_source_path: row
                .try_get("source_path")
                .context("source_path should be present")?,
            issues,
            summary,
        })
    } else {
        None
    };

    Ok(ConfigDraftDetail {
        draft_id: row.try_get("draft_id").context("draft_id should be present")?,
        cluster_id: row.try_get("cluster_id").context("cluster_id should be present")?,
        title: row.try_get("title").context("title should be present")?,
        summary: row.try_get("summary").context("summary should be present")?,
        source_path: row.try_get("source_path").context("source_path should be present")?,
        config_text: row.try_get("config_text").context("config_text should be present")?,
        base_revision_id: row
            .try_get("base_revision_id")
            .context("base_revision_id should be readable")?,
        validation_state: state,
        published_revision_id: row
            .try_get("published_revision_id")
            .context("published_revision_id should be readable")?,
        created_by: row.try_get("created_by").context("created_by should be present")?,
        updated_by: row.try_get("updated_by").context("updated_by should be present")?,
        created_at_unix_ms: unix_time_ms(
            row.try_get::<DateTime<Utc>, _>("created_at")
                .context("created_at should be present")?,
        )?,
        updated_at_unix_ms: unix_time_ms(
            row.try_get::<DateTime<Utc>, _>("updated_at")
                .context("updated_at should be present")?,
        )?,
        last_validation,
    })
}

fn map_node_row(row: sqlx::postgres::PgRow) -> Result<NodeSummary> {
    let state = row.try_get::<String, _>("state").context("node state should be present")?;
    let last_snapshot_version: Option<i64> =
        row.try_get("last_snapshot_version").context("last_snapshot_version should be readable")?;
    let runtime_revision: Option<i64> =
        row.try_get("runtime_revision").context("runtime_revision should be readable")?;
    let runtime_pid: Option<i32> =
        row.try_get("runtime_pid").context("runtime_pid should be readable")?;
    let listener_count: Option<i32> =
        row.try_get("listener_count").context("listener_count should be readable")?;
    let active_connections: Option<i32> =
        row.try_get("active_connections").context("active_connections should be readable")?;

    Ok(NodeSummary {
        node_id: row.try_get("node_id").context("node_id should be present")?,
        cluster_id: row.try_get("cluster_id").context("cluster_id should be present")?,
        advertise_addr: row
            .try_get("advertise_addr")
            .context("advertise_addr should be present")?,
        role: row.try_get("role").context("role should be present")?,
        state: state
            .parse()
            .map_err(|error: String| anyhow::anyhow!(error))
            .with_context(|| format!("invalid node state `{state}` loaded from postgres"))?,
        running_version: row
            .try_get("running_version")
            .context("running_version should be present")?,
        admin_socket_path: row
            .try_get("admin_socket_path")
            .context("admin_socket_path should be present")?,
        last_seen_unix_ms: unix_time_ms(
            row.try_get::<DateTime<Utc>, _>("observed_at")
                .context("observed_at should be present")?,
        )?,
        last_snapshot_version: option_i64_to_u64(last_snapshot_version, "last_snapshot_version")?,
        runtime_revision: option_i64_to_u64(runtime_revision, "runtime_revision")?,
        runtime_pid: option_i32_to_u32(runtime_pid, "runtime_pid")?,
        listener_count: option_i32_to_u32(listener_count, "listener_count")?,
        active_connections: option_i32_to_u32(active_connections, "active_connections")?,
        status_reason: row.try_get("status_reason").context("status_reason should be readable")?,
    })
}

fn map_node_snapshot_meta_row(row: sqlx::postgres::PgRow) -> Result<NodeSnapshotMeta> {
    let snapshot_version: i64 =
        row.try_get("snapshot_version").context("snapshot_version should be present")?;
    let schema_version: i32 =
        row.try_get("schema_version").context("schema_version should be present")?;
    let pid: i32 = row.try_get("pid").context("pid should be present")?;
    let included_modules: Value =
        row.try_get("included_modules").context("included_modules should be present")?;

    Ok(NodeSnapshotMeta {
        node_id: row.try_get("node_id").context("node_id should be present")?,
        snapshot_version: u64::try_from(snapshot_version)
            .context("snapshot_version should fit into u64")?,
        schema_version: u32::try_from(schema_version)
            .context("schema_version should fit into u32")?,
        captured_at_unix_ms: unix_time_ms(
            row.try_get::<DateTime<Utc>, _>("captured_at")
                .context("captured_at should be present")?,
        )?,
        pid: u32::try_from(pid).context("pid should fit into u32")?,
        binary_version: row
            .try_get("binary_version")
            .context("binary_version should be present")?,
        included_modules: json_array_to_strings(included_modules, "included_modules")?,
    })
}

fn map_node_snapshot_detail_row(row: sqlx::postgres::PgRow) -> Result<NodeSnapshotDetail> {
    let snapshot_version: i64 =
        row.try_get("snapshot_version").context("snapshot_version should be present")?;
    let schema_version: i32 =
        row.try_get("schema_version").context("schema_version should be present")?;
    let pid: i32 = row.try_get("pid").context("pid should be present")?;
    let included_modules: Value =
        row.try_get("included_modules").context("included_modules should be present")?;

    Ok(NodeSnapshotDetail {
        node_id: row.try_get("node_id").context("node_id should be present")?,
        snapshot_version: u64::try_from(snapshot_version)
            .context("snapshot_version should fit into u64")?,
        schema_version: u32::try_from(schema_version)
            .context("schema_version should fit into u32")?,
        captured_at_unix_ms: unix_time_ms(
            row.try_get::<DateTime<Utc>, _>("captured_at")
                .context("captured_at should be present")?,
        )?,
        pid: u32::try_from(pid).context("pid should fit into u32")?,
        binary_version: row
            .try_get("binary_version")
            .context("binary_version should be present")?,
        included_modules: json_array_to_strings(included_modules, "included_modules")?,
        status: row.try_get("status").context("status should be readable")?,
        counters: row.try_get("counters").context("counters should be readable")?,
        traffic: row.try_get("traffic").context("traffic should be readable")?,
        peer_health: row.try_get("peer_health").context("peer_health should be readable")?,
        upstreams: row.try_get("upstreams").context("upstreams should be readable")?,
    })
}

fn map_deployment_row(row: sqlx::postgres::PgRow) -> Result<DeploymentSummary> {
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
        status: status.parse().map_err(|error: String| anyhow::anyhow!(error)).with_context(
            || format!("invalid deployment status `{status}` loaded from postgres"),
        )?,
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

fn map_audit_row(row: sqlx::postgres::PgRow) -> Result<AuditLogSummary> {
    Ok(AuditLogSummary {
        audit_id: row.try_get("audit_id").context("audit_id should be present")?,
        request_id: row.try_get("request_id").context("request_id should be present")?,
        cluster_id: row.try_get("cluster_id").context("cluster_id should be readable")?,
        actor_id: row.try_get("actor_id").context("actor_id should be present")?,
        action: row.try_get("action").context("action should be present")?,
        resource_type: row.try_get("resource_type").context("resource_type should be present")?,
        resource_id: row.try_get("resource_id").context("resource_id should be present")?,
        result: row.try_get("result").context("result should be present")?,
        created_at_unix_ms: unix_time_ms(
            row.try_get::<DateTime<Utc>, _>("created_at")
                .context("created_at should be present")?,
        )?,
    })
}

fn map_audit_entry_row(row: sqlx::postgres::PgRow) -> Result<AuditLogEntry> {
    Ok(AuditLogEntry {
        audit_id: row.try_get("audit_id").context("audit_id should be present")?,
        request_id: row.try_get("request_id").context("request_id should be present")?,
        cluster_id: row.try_get("cluster_id").context("cluster_id should be readable")?,
        actor_id: row.try_get("actor_id").context("actor_id should be present")?,
        action: row.try_get("action").context("action should be present")?,
        resource_type: row.try_get("resource_type").context("resource_type should be present")?,
        resource_id: row.try_get("resource_id").context("resource_id should be present")?,
        result: row.try_get("result").context("result should be present")?,
        details: row.try_get("details").context("details should be readable")?,
        created_at_unix_ms: unix_time_ms(
            row.try_get::<DateTime<Utc>, _>("created_at")
                .context("created_at should be present")?,
        )?,
    })
}

fn count_to_u32(value: i64, field: &str) -> Result<u32> {
    u32::try_from(value).with_context(|| format!("{field} should fit into u32"))
}

fn count_to_u64(value: i64, field: &str) -> Result<u64> {
    u64::try_from(value).with_context(|| format!("{field} should fit into u64"))
}

fn count_to_usize(value: i64, field: &str) -> Result<usize> {
    usize::try_from(value).with_context(|| format!("{field} should fit into usize"))
}

fn option_u64_to_i64(value: Option<u64>, field: &str) -> Result<Option<i64>> {
    value
        .map(|value| i64::try_from(value).with_context(|| format!("{field} should fit into i64")))
        .transpose()
}

fn option_u32_to_i32(value: Option<u32>, field: &str) -> Result<Option<i32>> {
    value
        .map(|value| i32::try_from(value).with_context(|| format!("{field} should fit into i32")))
        .transpose()
}

fn option_i64_to_u64(value: Option<i64>, field: &str) -> Result<Option<u64>> {
    value
        .map(|value| u64::try_from(value).with_context(|| format!("{field} should fit into u64")))
        .transpose()
}

fn option_i32_to_u32(value: Option<i32>, field: &str) -> Result<Option<u32>> {
    value
        .map(|value| u32::try_from(value).with_context(|| format!("{field} should fit into u32")))
        .transpose()
}

fn unix_time_ms(value: DateTime<Utc>) -> Result<u64> {
    u64::try_from(value.timestamp_millis()).context("timestamp should fit into unix milliseconds")
}

fn utc_from_unix_ms(value: u64) -> Result<DateTime<Utc>> {
    let value = i64::try_from(value).context("unix milliseconds should fit into i64")?;
    DateTime::<Utc>::from_timestamp_millis(value)
        .context("unix milliseconds should produce a valid UTC timestamp")
}

fn heartbeat_payload(report: &NodeAgentRegistrationRequest) -> Value {
    serde_json::json!({
        "advertise_addr": report.advertise_addr,
        "role": report.role,
        "running_version": report.running_version,
        "snapshot_version": report.runtime.snapshot_version,
        "revision": report.runtime.revision,
        "pid": report.runtime.pid,
        "listener_count": report.runtime.listener_count,
        "active_connections": report.runtime.active_connections,
        "error": report.runtime.error,
    })
}

fn snapshot_payload(snapshot: &NodeSnapshotIngestRequest) -> Value {
    serde_json::json!({
        "snapshot_version": snapshot.snapshot_version,
        "schema_version": snapshot.schema_version,
        "captured_at_unix_ms": snapshot.captured_at_unix_ms,
        "pid": snapshot.pid,
        "binary_version": snapshot.binary_version,
        "included_modules": snapshot.included_modules,
        "status": snapshot.status,
        "counters": snapshot.counters,
        "traffic": snapshot.traffic,
        "peer_health": snapshot.peer_health,
        "upstreams": snapshot.upstreams,
    })
}

fn compile_summary_value(summary: Option<&ConfigCompileSummary>) -> Result<Value> {
    match summary {
        Some(summary) => serde_json::to_value(summary).context("failed to encode compile summary"),
        None => Ok(serde_json::json!({})),
    }
}

fn parse_compile_summary_value(value: Value) -> Result<Option<ConfigCompileSummary>> {
    if value.is_null() || value.as_object().is_some_and(|object| object.is_empty()) {
        return Ok(None);
    }

    serde_json::from_value(value)
        .map(Some)
        .context("failed to decode compile summary from postgres")
}

fn json_array_to_strings(value: Value, field: &str) -> Result<Vec<String>> {
    let array = value.as_array().with_context(|| format!("{field} should be a JSON array"))?;
    array
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .map(ToOwned::to_owned)
                .with_context(|| format!("{field} entries should be strings"))
        })
        .collect()
}
