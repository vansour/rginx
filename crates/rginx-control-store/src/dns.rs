use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Row, postgres::PgRow};

use rginx_control_types::{
    DnsDraftDetail, DnsDraftSummary, DnsDraftValidationState, DnsPlan, DnsRevisionDetail,
    DnsRevisionListItem, DnsRuntimeStatus, DnsValidationReport,
};

use crate::repositories::{ControlPlaneStore, NewAuditLogEntry};

#[derive(Debug, Clone)]
pub struct NewDnsDraftRecord {
    pub draft_id: String,
    pub cluster_id: String,
    pub title: String,
    pub summary: String,
    pub plan: DnsPlan,
    pub base_revision_id: Option<String>,
    pub created_by: String,
    pub updated_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct UpdateDnsDraftRecord {
    pub title: String,
    pub summary: String,
    pub plan: DnsPlan,
    pub base_revision_id: Option<String>,
    pub updated_by: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct DraftDnsValidationRecord {
    pub validation_state: DnsDraftValidationState,
    pub validation_report: DnsValidationReport,
    pub updated_by: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewDnsRevisionRecord {
    pub revision_id: String,
    pub cluster_id: String,
    pub version_label: String,
    pub summary: String,
    pub plan: DnsPlan,
    pub validation: DnsValidationReport,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub published_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct DnsRepository {
    store: ControlPlaneStore,
}

impl DnsRepository {
    pub fn new(store: ControlPlaneStore) -> Self {
        Self { store }
    }

    pub async fn list_revisions(&self) -> Result<Vec<DnsRevisionListItem>> {
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
            order by created_at desc, revision_id desc
            "#,
        )
        .fetch_all(self.store.postgres())
        .await
        .context("failed to list dns revisions from postgres")?
        .into_iter()
        .map(map_dns_revision_list_row)
        .collect()
    }

    pub async fn load_revision_detail(
        &self,
        revision_id: &str,
    ) -> Result<Option<DnsRevisionDetail>> {
        sqlx::query(
            r#"
            select
                revision_id,
                cluster_id,
                version_label,
                summary,
                plan_json,
                validation_summary,
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
        .with_context(|| format!("failed to load dns revision `{revision_id}` from postgres"))?
        .map(map_dns_revision_detail_row)
        .transpose()
    }

    pub async fn load_latest_revision_for_cluster(
        &self,
        cluster_id: &str,
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
            where cluster_id = $1
            order by created_at desc, revision_id desc
            limit 1
            "#,
        )
        .bind(cluster_id)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| format!("failed to load latest dns revision for cluster `{cluster_id}`"))?
        .map(map_dns_revision_list_row)
        .transpose()
    }

    pub async fn list_drafts(&self) -> Result<Vec<DnsDraftSummary>> {
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
            from cp_dns_drafts
            order by updated_at desc, draft_id desc
            "#,
        )
        .fetch_all(self.store.postgres())
        .await
        .context("failed to list dns drafts from postgres")?
        .into_iter()
        .map(map_dns_draft_summary_row)
        .collect()
    }

    pub async fn load_draft_detail(&self, draft_id: &str) -> Result<Option<DnsDraftDetail>> {
        sqlx::query(
            r#"
            select
                draft_id,
                cluster_id,
                title,
                summary,
                plan_json,
                base_revision_id,
                validation_state,
                validation_summary,
                validated_at,
                published_revision_id,
                created_by,
                updated_by,
                created_at,
                updated_at
            from cp_dns_drafts
            where draft_id = $1
            "#,
        )
        .bind(draft_id)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| format!("failed to load dns draft `{draft_id}` from postgres"))?
        .map(map_dns_draft_detail_row)
        .transpose()
    }

    pub async fn create_draft_with_audit(
        &self,
        draft: &NewDnsDraftRecord,
        audit: &NewAuditLogEntry,
    ) -> Result<DnsDraftDetail> {
        let mut transaction =
            self.store.postgres().begin().await.context("failed to start dns draft transaction")?;

        sqlx::query(
            r#"
            insert into cp_dns_drafts (
                draft_id,
                cluster_id,
                title,
                summary,
                plan_json,
                base_revision_id,
                validation_state,
                validation_summary,
                validated_at,
                published_revision_id,
                created_by,
                updated_by,
                created_at,
                updated_at
            )
            values (
                $1, $2, $3, $4, $5, $6, 'pending', null, null, null, $7, $8, $9, $10
            )
            "#,
        )
        .bind(&draft.draft_id)
        .bind(&draft.cluster_id)
        .bind(&draft.title)
        .bind(&draft.summary)
        .bind(plan_value(&draft.plan)?)
        .bind(&draft.base_revision_id)
        .bind(&draft.created_by)
        .bind(&draft.updated_by)
        .bind(draft.created_at)
        .bind(draft.updated_at)
        .execute(&mut *transaction)
        .await
        .with_context(|| format!("failed to insert dns draft `{}`", draft.draft_id))?;

        insert_audit_entry(&mut transaction, audit).await?;
        transaction.commit().await.context("failed to commit dns draft transaction")?;

        self.load_draft_detail(&draft.draft_id).await?.ok_or_else(|| {
            anyhow!("dns draft `{}` disappeared after insert commit", draft.draft_id)
        })
    }

    pub async fn update_draft_with_audit(
        &self,
        draft_id: &str,
        update: &UpdateDnsDraftRecord,
        audit: &NewAuditLogEntry,
    ) -> Result<Option<DnsDraftDetail>> {
        let mut transaction = self
            .store
            .postgres()
            .begin()
            .await
            .context("failed to start dns draft update transaction")?;

        let result = sqlx::query(
            r#"
            update cp_dns_drafts
            set
                title = $2,
                summary = $3,
                plan_json = $4,
                base_revision_id = $5,
                validation_state = 'pending',
                validation_summary = null,
                validated_at = null,
                published_revision_id = null,
                updated_by = $6,
                updated_at = $7
            where draft_id = $1
            "#,
        )
        .bind(draft_id)
        .bind(&update.title)
        .bind(&update.summary)
        .bind(plan_value(&update.plan)?)
        .bind(&update.base_revision_id)
        .bind(&update.updated_by)
        .bind(update.updated_at)
        .execute(&mut *transaction)
        .await
        .with_context(|| format!("failed to update dns draft `{draft_id}`"))?;

        if result.rows_affected() == 0 {
            transaction.rollback().await.context("failed to rollback missing dns draft update")?;
            return Ok(None);
        }

        insert_audit_entry(&mut transaction, audit).await?;
        transaction.commit().await.context("failed to commit dns draft update transaction")?;

        self.load_draft_detail(draft_id).await
    }

    pub async fn store_validation_with_audit(
        &self,
        draft_id: &str,
        validation: &DraftDnsValidationRecord,
        audit: &NewAuditLogEntry,
    ) -> Result<Option<DnsDraftDetail>> {
        let mut transaction = self
            .store
            .postgres()
            .begin()
            .await
            .context("failed to start dns validation transaction")?;

        let result = sqlx::query(
            r#"
            update cp_dns_drafts
            set
                validation_state = $2,
                validation_summary = $3,
                validated_at = $4,
                updated_by = $5,
                updated_at = $6
            where draft_id = $1
            "#,
        )
        .bind(draft_id)
        .bind(validation.validation_state.as_str())
        .bind(validation_report_value(&validation.validation_report)?)
        .bind(utc_from_unix_ms(validation.validation_report.validated_at_unix_ms)?)
        .bind(&validation.updated_by)
        .bind(validation.updated_at)
        .execute(&mut *transaction)
        .await
        .with_context(|| format!("failed to store dns draft validation for `{draft_id}`"))?;

        if result.rows_affected() == 0 {
            transaction
                .rollback()
                .await
                .context("failed to rollback missing dns validation update")?;
            return Ok(None);
        }

        insert_audit_entry(&mut transaction, audit).await?;
        transaction.commit().await.context("failed to commit dns validation transaction")?;

        self.load_draft_detail(draft_id).await
    }

    pub async fn publish_draft_with_audit(
        &self,
        draft_id: &str,
        revision: &NewDnsRevisionRecord,
        audit: &NewAuditLogEntry,
    ) -> Result<Option<(DnsDraftDetail, DnsRevisionDetail)>> {
        let mut transaction = self
            .store
            .postgres()
            .begin()
            .await
            .context("failed to start dns publish transaction")?;

        sqlx::query(
            r#"
            insert into cp_dns_revisions (
                revision_id,
                cluster_id,
                version_label,
                summary,
                plan_json,
                validation_summary,
                created_by,
                created_at,
                published_at
            )
            values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(&revision.revision_id)
        .bind(&revision.cluster_id)
        .bind(&revision.version_label)
        .bind(&revision.summary)
        .bind(plan_value(&revision.plan)?)
        .bind(validation_report_value(&revision.validation)?)
        .bind(&revision.created_by)
        .bind(revision.created_at)
        .bind(revision.published_at)
        .execute(&mut *transaction)
        .await
        .with_context(|| format!("failed to insert dns revision `{}`", revision.revision_id))?;

        let result = sqlx::query(
            r#"
            update cp_dns_drafts
            set
                validation_state = 'published',
                validation_summary = $3,
                validated_at = $4,
                published_revision_id = $2,
                updated_by = $5,
                updated_at = $4
            where draft_id = $1
            "#,
        )
        .bind(draft_id)
        .bind(&revision.revision_id)
        .bind(validation_report_value(&revision.validation)?)
        .bind(revision.published_at)
        .bind(&revision.created_by)
        .execute(&mut *transaction)
        .await
        .with_context(|| format!("failed to mark dns draft `{draft_id}` as published"))?;

        if result.rows_affected() == 0 {
            transaction
                .rollback()
                .await
                .context("failed to rollback missing dns draft publish update")?;
            return Ok(None);
        }

        insert_audit_entry(&mut transaction, audit).await?;
        transaction.commit().await.context("failed to commit dns publish transaction")?;

        let draft = self
            .load_draft_detail(draft_id)
            .await?
            .ok_or_else(|| anyhow!("dns draft `{draft_id}` disappeared after publish commit"))?;
        let revision =
            self.load_revision_detail(&revision.revision_id).await?.ok_or_else(|| {
                anyhow!("dns revision `{}` disappeared after publish commit", revision.revision_id)
            })?;
        Ok(Some((draft, revision)))
    }

    pub async fn load_runtime_status(&self) -> Result<Vec<DnsRuntimeStatus>> {
        sqlx::query(
            r#"
            select
                s.cluster_id,
                s.published_revision_id,
                r.version_label,
                r.plan_json
            from cp_dns_runtime_state s
            left join cp_dns_revisions r on r.revision_id = s.published_revision_id
            order by s.cluster_id asc
            "#,
        )
        .fetch_all(self.store.postgres())
        .await
        .context("failed to load dns runtime status from postgres")?
        .into_iter()
        .map(map_dns_runtime_row)
        .collect()
    }

    pub async fn load_published_revision_for_cluster(
        &self,
        cluster_id: &str,
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
            from cp_dns_runtime_state s
            join cp_dns_revisions r on r.revision_id = s.published_revision_id
            where s.cluster_id = $1
            "#,
        )
        .bind(cluster_id)
        .fetch_optional(self.store.postgres())
        .await
        .with_context(|| {
            format!("failed to load published dns revision for cluster `{cluster_id}`")
        })?
        .map(map_dns_revision_detail_row)
        .transpose()
    }

    pub async fn load_all_published_revisions(&self) -> Result<Vec<DnsRevisionDetail>> {
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
            from cp_dns_runtime_state s
            join cp_dns_revisions r on r.revision_id = s.published_revision_id
            order by r.cluster_id asc
            "#,
        )
        .fetch_all(self.store.postgres())
        .await
        .context("failed to load all published dns revisions from postgres")?
        .into_iter()
        .map(map_dns_revision_detail_row)
        .collect()
    }
}

async fn insert_audit_entry(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
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
    let plan = row.try_get::<Value, _>("plan_json").context("plan_json should be readable")?;
    let validation = row
        .try_get::<Value, _>("validation_summary")
        .context("validation_summary should be readable")?;

    Ok(DnsRevisionDetail {
        revision_id: row.try_get("revision_id").context("revision_id should be present")?,
        cluster_id: row.try_get("cluster_id").context("cluster_id should be present")?,
        version_label: row.try_get("version_label").context("version_label should be present")?,
        summary: row.try_get("summary").context("summary should be present")?,
        plan: parse_plan_value(plan)?,
        validation: parse_validation_value(validation)?,
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

fn map_dns_draft_summary_row(row: PgRow) -> Result<DnsDraftSummary> {
    let validation_state = row
        .try_get::<String, _>("validation_state")
        .context("validation_state should be present")?;

    Ok(DnsDraftSummary {
        draft_id: row.try_get("draft_id").context("draft_id should be present")?,
        cluster_id: row.try_get("cluster_id").context("cluster_id should be present")?,
        title: row.try_get("title").context("title should be present")?,
        summary: row.try_get("summary").context("summary should be present")?,
        base_revision_id: row
            .try_get("base_revision_id")
            .context("base_revision_id should be readable")?,
        validation_state: validation_state
            .parse()
            .map_err(|error: String| anyhow!(error))
            .with_context(|| {
                format!("invalid dns validation_state `{validation_state}` loaded from postgres")
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

fn map_dns_draft_detail_row(row: PgRow) -> Result<DnsDraftDetail> {
    let validation_state = row
        .try_get::<String, _>("validation_state")
        .context("validation_state should be present")?;
    let validation_summary = row
        .try_get::<Option<Value>, _>("validation_summary")
        .context("validation_summary should be readable")?;
    let validated_at = row
        .try_get::<Option<DateTime<Utc>>, _>("validated_at")
        .context("validated_at should be readable")?;
    let state =
        validation_state.parse().map_err(|error: String| anyhow!(error)).with_context(|| {
            format!("invalid dns validation_state `{validation_state}` loaded from postgres")
        })?;

    let last_validation = match (validation_summary, validated_at) {
        (Some(summary), _) => Some(parse_validation_value(summary)?),
        (None, Some(validated_at)) => Some(DnsValidationReport {
            valid: matches!(
                state,
                DnsDraftValidationState::Valid | DnsDraftValidationState::Published
            ),
            validated_at_unix_ms: unix_time_ms(validated_at)?,
            issues: Vec::new(),
            zone_count: 0,
            record_count: 0,
            target_count: 0,
        }),
        (None, None) => None,
    };

    Ok(DnsDraftDetail {
        draft_id: row.try_get("draft_id").context("draft_id should be present")?,
        cluster_id: row.try_get("cluster_id").context("cluster_id should be present")?,
        title: row.try_get("title").context("title should be present")?,
        summary: row.try_get("summary").context("summary should be present")?,
        plan: parse_plan_value(
            row.try_get::<Value, _>("plan_json").context("plan_json should be readable")?,
        )?,
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

fn map_dns_runtime_row(row: PgRow) -> Result<DnsRuntimeStatus> {
    let (zone_count, record_count) = row
        .try_get::<Option<Value>, _>("plan_json")
        .context("plan_json should be readable")?
        .map(parse_plan_value)
        .transpose()?
        .map(|plan| {
            let zone_count = u32::try_from(plan.zones.len()).unwrap_or(u32::MAX);
            let record_count =
                u32::try_from(plan.zones.iter().map(|zone| zone.records.len()).sum::<usize>())
                    .unwrap_or(u32::MAX);
            (zone_count, record_count)
        })
        .unwrap_or((0, 0));

    Ok(DnsRuntimeStatus {
        enabled: true,
        cluster_id: row.try_get("cluster_id").context("cluster_id should be readable")?,
        udp_bind_addr: None,
        tcp_bind_addr: None,
        published_revision_id: row
            .try_get("published_revision_id")
            .context("published_revision_id should be readable")?,
        published_revision_version: row
            .try_get("version_label")
            .context("version_label should be readable")?,
        zone_count,
        record_count,
        query_total: 0,
        response_noerror_total: 0,
        response_nxdomain_total: 0,
        response_servfail_total: 0,
        hot_queries: Vec::new(),
        error_queries: Vec::new(),
    })
}

fn plan_value(plan: &DnsPlan) -> Result<Value> {
    serde_json::to_value(plan).context("failed to encode dns plan")
}

fn validation_report_value(report: &DnsValidationReport) -> Result<Value> {
    serde_json::to_value(report).context("failed to encode dns validation report")
}

fn parse_plan_value(value: Value) -> Result<DnsPlan> {
    serde_json::from_value(value).context("failed to decode dns plan from postgres")
}

fn parse_validation_value(value: Value) -> Result<DnsValidationReport> {
    serde_json::from_value(value).context("failed to decode dns validation report from postgres")
}

fn unix_time_ms(value: DateTime<Utc>) -> Result<u64> {
    u64::try_from(value.timestamp_millis()).context("timestamp should fit into unix milliseconds")
}

fn utc_from_unix_ms(value: u64) -> Result<DateTime<Utc>> {
    let value = i64::try_from(value).context("unix milliseconds should fit into i64")?;
    DateTime::<Utc>::from_timestamp_millis(value)
        .context("unix milliseconds should produce a valid UTC timestamp")
}
