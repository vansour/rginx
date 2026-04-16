use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;
use serde_json::json;

use rginx_config::ConfigSnapshot;
use rginx_control_store::{
    ControlPlaneStore, DraftValidationRecord, NewAuditLogEntry, NewConfigDraftRecord,
    NewConfigRevisionRecord, UpdateConfigDraftRecord,
};
use rginx_control_types::{
    AuthenticatedActor, CompiledListenerBindingSummary, CompiledListenerSummary,
    CompiledTlsSummary, ConfigCompileSummary, ConfigDiffLine, ConfigDiffLineKind,
    ConfigDiffResponse, ConfigDraftDetail, ConfigDraftSummary, ConfigDraftValidationState,
    ConfigRevisionDetail, ConfigRevisionListItem, ConfigValidationReport, CreateConfigDraftRequest,
    PublishConfigDraftRequest, PublishConfigDraftResponse, UpdateConfigDraftRequest,
};

use crate::{ServiceError, ServiceResult};

static REVISION_EVENT_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct RevisionService {
    store: ControlPlaneStore,
}

impl RevisionService {
    pub fn new(store: ControlPlaneStore) -> Self {
        Self { store }
    }

    pub async fn list_revisions(&self) -> ServiceResult<Vec<ConfigRevisionListItem>> {
        self.store
            .revision_repository()
            .list_revisions()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))
    }

    pub async fn get_revision(&self, revision_id: &str) -> ServiceResult<ConfigRevisionDetail> {
        self.store
            .revision_repository()
            .load_revision_detail(revision_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .ok_or_else(|| {
                ServiceError::NotFound(format!("revision `{revision_id}` was not found"))
            })
    }

    pub async fn list_drafts(&self) -> ServiceResult<Vec<ConfigDraftSummary>> {
        self.store
            .revision_repository()
            .list_drafts()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))
    }

    pub async fn get_draft(&self, draft_id: &str) -> ServiceResult<ConfigDraftDetail> {
        self.store
            .revision_repository()
            .load_draft_detail(draft_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .ok_or_else(|| {
                ServiceError::NotFound(format!("config draft `{draft_id}` was not found"))
            })
    }

    pub async fn create_draft(
        &self,
        actor: &AuthenticatedActor,
        request: CreateConfigDraftRequest,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> ServiceResult<ConfigDraftDetail> {
        let draft_id = self.generate_id("draft");
        self.validate_draft_input(
            &request.cluster_id,
            &request.title,
            &request.summary,
            &request.source_path,
            request.base_revision_id.as_deref(),
        )
        .await?;

        let now = Utc::now();
        let draft = self
            .store
            .revision_repository()
            .create_draft_with_audit(
                &NewConfigDraftRecord {
                    draft_id: draft_id.clone(),
                    cluster_id: request.cluster_id.clone(),
                    title: request.title.clone(),
                    summary: request.summary.clone(),
                    source_path: request.source_path.clone(),
                    config_text: request.config_text,
                    base_revision_id: request.base_revision_id.clone(),
                    created_by: actor.user.username.clone(),
                    updated_by: actor.user.username.clone(),
                    created_at: now,
                    updated_at: now,
                },
                &NewAuditLogEntry {
                    audit_id: self.generate_id("audit"),
                    request_id: request_id.to_string(),
                    cluster_id: Some(request.cluster_id),
                    actor_id: actor.user.user_id.clone(),
                    action: "revision.draft_created".to_string(),
                    resource_type: "config_draft".to_string(),
                    resource_id: draft_id,
                    result: "succeeded".to_string(),
                    details: json!({
                        "title": request.title,
                        "summary": request.summary,
                        "source_path": request.source_path,
                        "base_revision_id": request.base_revision_id,
                        "user_agent": user_agent,
                        "remote_addr": remote_addr,
                    }),
                    created_at: now,
                },
            )
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;

        Ok(draft)
    }

    pub async fn update_draft(
        &self,
        actor: &AuthenticatedActor,
        draft_id: &str,
        request: UpdateConfigDraftRequest,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> ServiceResult<ConfigDraftDetail> {
        let existing = self.get_draft(draft_id).await?;
        self.validate_draft_input(
            &existing.cluster_id,
            &request.title,
            &request.summary,
            &request.source_path,
            request.base_revision_id.as_deref(),
        )
        .await?;

        self.store
            .revision_repository()
            .update_draft_with_audit(
                draft_id,
                &UpdateConfigDraftRecord {
                    title: request.title.clone(),
                    summary: request.summary.clone(),
                    source_path: request.source_path.clone(),
                    config_text: request.config_text,
                    base_revision_id: request.base_revision_id.clone(),
                    updated_by: actor.user.username.clone(),
                    updated_at: Utc::now(),
                },
                &NewAuditLogEntry {
                    audit_id: self.generate_id("audit"),
                    request_id: request_id.to_string(),
                    cluster_id: Some(existing.cluster_id.clone()),
                    actor_id: actor.user.user_id.clone(),
                    action: "revision.draft_updated".to_string(),
                    resource_type: "config_draft".to_string(),
                    resource_id: draft_id.to_string(),
                    result: "succeeded".to_string(),
                    details: json!({
                        "title": request.title,
                        "summary": request.summary,
                        "source_path": request.source_path,
                        "base_revision_id": request.base_revision_id,
                        "published_revision_id_cleared": existing.published_revision_id,
                        "user_agent": user_agent,
                        "remote_addr": remote_addr,
                    }),
                    created_at: Utc::now(),
                },
            )
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .ok_or_else(|| {
                ServiceError::NotFound(format!("config draft `{draft_id}` was not found"))
            })
    }

    pub async fn validate_draft(
        &self,
        actor: &AuthenticatedActor,
        draft_id: &str,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> ServiceResult<ConfigDraftDetail> {
        let draft = self.get_draft(draft_id).await?;
        let report = self.build_validation_report(&draft.source_path, &draft.config_text)?;

        self.persist_validation(
            actor,
            &draft,
            request_id,
            user_agent,
            remote_addr,
            "revision.draft_validated",
            report,
        )
        .await
    }

    pub async fn diff_draft(
        &self,
        draft_id: &str,
        target_revision_id: Option<&str>,
    ) -> ServiceResult<ConfigDiffResponse> {
        let draft = self.get_draft(draft_id).await?;
        let target = match target_revision_id {
            Some(revision_id) => self.get_revision(revision_id).await?,
            None => match draft.base_revision_id.as_deref() {
                Some(revision_id) => self.get_revision(revision_id).await?,
                None => {
                    if let Some(revision) = self
                        .store
                        .revision_repository()
                        .load_latest_revision_for_cluster(&draft.cluster_id)
                        .await
                        .map_err(|error| ServiceError::Internal(error.to_string()))?
                    {
                        self.get_revision(&revision.revision_id).await?
                    } else {
                        return Ok(build_diff_response(
                            "empty revision".to_string(),
                            "",
                            format!("draft {}", draft.title),
                            &draft.config_text,
                        ));
                    }
                }
            },
        };

        Ok(build_diff_response(
            format!("revision {}", target.version_label),
            &target.config_text,
            format!("draft {}", draft.title),
            &draft.config_text,
        ))
    }

    pub async fn publish_draft(
        &self,
        actor: &AuthenticatedActor,
        draft_id: &str,
        request: PublishConfigDraftRequest,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> ServiceResult<PublishConfigDraftResponse> {
        let draft = self.get_draft(draft_id).await?;
        if request.version_label.trim().is_empty() {
            return Err(ServiceError::BadRequest("version_label should not be empty".to_string()));
        }

        let report = self.build_validation_report(&draft.source_path, &draft.config_text)?;
        if !report.valid {
            let _ = self
                .persist_validation(
                    actor,
                    &draft,
                    request_id,
                    user_agent.clone(),
                    remote_addr.clone(),
                    "revision.publish_validation_failed",
                    report.clone(),
                )
                .await;
            return Err(ServiceError::BadRequest(
                "draft is invalid; validate and fix the config before publishing".to_string(),
            ));
        }

        let now = Utc::now();
        let revision_id = self.generate_id("rev");
        let published_summary = request.summary.unwrap_or_else(|| draft.summary.clone());
        let published = self
            .store
            .revision_repository()
            .publish_draft_with_audit(
                draft_id,
                &NewConfigRevisionRecord {
                    revision_id: revision_id.clone(),
                    cluster_id: draft.cluster_id.clone(),
                    version_label: request.version_label.clone(),
                    summary: published_summary.clone(),
                    source_path: draft.source_path.clone(),
                    config_text: draft.config_text.clone(),
                    compile_summary: report.summary.clone(),
                    created_by: actor.user.username.clone(),
                    created_at: now,
                },
                &NewAuditLogEntry {
                    audit_id: self.generate_id("audit"),
                    request_id: request_id.to_string(),
                    cluster_id: Some(draft.cluster_id.clone()),
                    actor_id: actor.user.user_id.clone(),
                    action: "revision.published".to_string(),
                    resource_type: "revision".to_string(),
                    resource_id: revision_id,
                    result: "succeeded".to_string(),
                    details: json!({
                        "draft_id": draft.draft_id,
                        "title": draft.title,
                        "version_label": request.version_label,
                        "summary": published_summary,
                        "source_path": draft.source_path,
                        "base_revision_id": draft.base_revision_id,
                        "user_agent": user_agent,
                        "remote_addr": remote_addr,
                    }),
                    created_at: now,
                },
            )
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .ok_or_else(|| {
                ServiceError::NotFound(format!("config draft `{draft_id}` was not found"))
            })?;

        Ok(PublishConfigDraftResponse { draft: published.0, revision: published.1 })
    }

    async fn validate_draft_input(
        &self,
        cluster_id: &str,
        title: &str,
        _summary: &str,
        source_path: &str,
        base_revision_id: Option<&str>,
    ) -> ServiceResult<()> {
        if cluster_id.trim().is_empty() {
            return Err(ServiceError::BadRequest("cluster_id should not be empty".to_string()));
        }
        if title.trim().is_empty() {
            return Err(ServiceError::BadRequest("title should not be empty".to_string()));
        }
        if source_path.trim().is_empty() {
            return Err(ServiceError::BadRequest("source_path should not be empty".to_string()));
        }
        if !self
            .store
            .revision_repository()
            .cluster_exists(cluster_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
        {
            return Err(ServiceError::BadRequest(format!("cluster `{cluster_id}` does not exist")));
        }
        if let Some(revision_id) = base_revision_id {
            let revision = self.get_revision(revision_id).await?;
            if revision.cluster_id != cluster_id {
                return Err(ServiceError::BadRequest(format!(
                    "base revision `{revision_id}` belongs to cluster `{}`, not `{cluster_id}`",
                    revision.cluster_id
                )));
            }
        }

        Ok(())
    }

    fn build_validation_report(
        &self,
        source_path: &str,
        config_text: &str,
    ) -> ServiceResult<ConfigValidationReport> {
        if config_text.trim().is_empty() {
            return Ok(ConfigValidationReport {
                valid: false,
                validated_at_unix_ms: unix_time_ms(SystemTime::now()),
                normalized_source_path: source_path.to_string(),
                issues: vec!["config_text should not be empty".to_string()],
                summary: None,
            });
        }

        match rginx_config::load_and_compile_from_str(config_text, Path::new(source_path)) {
            Ok(compiled) => Ok(ConfigValidationReport {
                valid: true,
                validated_at_unix_ms: unix_time_ms(SystemTime::now()),
                normalized_source_path: source_path.to_string(),
                issues: Vec::new(),
                summary: Some(build_compile_summary(&compiled)?),
            }),
            Err(error) => Ok(ConfigValidationReport {
                valid: false,
                validated_at_unix_ms: unix_time_ms(SystemTime::now()),
                normalized_source_path: source_path.to_string(),
                issues: vec![error.to_string()],
                summary: None,
            }),
        }
    }

    async fn persist_validation(
        &self,
        actor: &AuthenticatedActor,
        draft: &ConfigDraftDetail,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
        action: &str,
        report: ConfigValidationReport,
    ) -> ServiceResult<ConfigDraftDetail> {
        self.store
            .revision_repository()
            .store_validation_with_audit(
                &draft.draft_id,
                &DraftValidationRecord {
                    validation_state: if report.valid {
                        ConfigDraftValidationState::Valid
                    } else {
                        ConfigDraftValidationState::Invalid
                    },
                    validation_errors: report.issues.clone(),
                    compile_summary: report.summary.clone(),
                    validated_at: Some(Utc::now()),
                    updated_by: actor.user.username.clone(),
                    updated_at: Utc::now(),
                },
                &NewAuditLogEntry {
                    audit_id: self.generate_id("audit"),
                    request_id: request_id.to_string(),
                    cluster_id: Some(draft.cluster_id.clone()),
                    actor_id: actor.user.user_id.clone(),
                    action: action.to_string(),
                    resource_type: "config_draft".to_string(),
                    resource_id: draft.draft_id.clone(),
                    result: if report.valid { "succeeded" } else { "failed" }.to_string(),
                    details: json!({
                        "title": draft.title,
                        "source_path": draft.source_path,
                        "valid": report.valid,
                        "issues": report.issues,
                        "user_agent": user_agent,
                        "remote_addr": remote_addr,
                    }),
                    created_at: Utc::now(),
                },
            )
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .ok_or_else(|| {
                ServiceError::NotFound(format!("config draft `{}` was not found", draft.draft_id))
            })
    }

    fn generate_id(&self, prefix: &str) -> String {
        let now = unix_time_ms(SystemTime::now());
        let sequence = REVISION_EVENT_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{prefix}_{now}_{sequence}")
    }
}

fn build_compile_summary(config: &ConfigSnapshot) -> ServiceResult<ConfigCompileSummary> {
    let mut upstream_names = config.upstreams.keys().cloned().collect::<Vec<_>>();
    upstream_names.sort();

    let listeners = config
        .listeners
        .iter()
        .map(|listener| {
            Ok(CompiledListenerSummary {
                listener_id: listener.id.clone(),
                listener_name: listener.name.clone(),
                listen_addr: listener.server.listen_addr.to_string(),
                binding_count: usize_to_u32(listener.binding_count(), "binding_count")?,
                tls_enabled: listener.tls_enabled(),
                http3_enabled: listener.http3_enabled(),
                default_certificate: listener.server.default_certificate.clone(),
                bindings: listener
                    .transport_bindings()
                    .into_iter()
                    .map(|binding| CompiledListenerBindingSummary {
                        binding_name: binding.name.to_string(),
                        transport: binding.kind.as_str().to_string(),
                        listen_addr: binding.listen_addr.to_string(),
                        protocols: binding
                            .protocols
                            .into_iter()
                            .map(|protocol| protocol.as_str().to_string())
                            .collect(),
                    })
                    .collect(),
            })
        })
        .collect::<ServiceResult<Vec<_>>>()?;
    let default_certificates = config
        .listeners
        .iter()
        .filter_map(|listener| {
            listener
                .server
                .default_certificate
                .as_ref()
                .map(|certificate| format!("{}={certificate}", listener.name))
        })
        .collect::<Vec<_>>();

    Ok(ConfigCompileSummary {
        listener_model: listener_model(
            config.total_listener_count(),
            config.listeners.first().map(|listener| listener.id.as_str()),
            config.listeners.first().map(|listener| listener.name.as_str()),
        )
        .to_string(),
        listener_count: usize_to_u32(config.total_listener_count(), "listener_count")?,
        listener_binding_count: usize_to_u32(
            config.total_listener_binding_count(),
            "listener_binding_count",
        )?,
        total_vhost_count: usize_to_u32(config.total_vhost_count(), "total_vhost_count")?,
        total_route_count: usize_to_u32(config.total_route_count(), "total_route_count")?,
        upstream_count: usize_to_u32(config.upstreams.len(), "upstream_count")?,
        worker_threads: config
            .runtime
            .worker_threads
            .map(|value| usize_to_u32(value, "worker_threads"))
            .transpose()?,
        accept_workers: usize_to_u32(config.runtime.accept_workers, "accept_workers")?,
        tls_enabled: config.tls_enabled(),
        http3_enabled: config.http3_enabled(),
        http3_early_data_enabled_listeners: usize_to_u32(
            config
                .listeners
                .iter()
                .filter(|listener| {
                    listener.http3.as_ref().is_some_and(|http3| http3.early_data_enabled)
                })
                .count(),
            "http3_early_data_enabled_listeners",
        )?,
        default_server_names: config.default_vhost.server_names.clone(),
        upstream_names,
        listeners,
        tls: CompiledTlsSummary {
            listener_tls_profiles: usize_to_u32(
                config.listeners.iter().filter(|listener| listener.server.tls.is_some()).count(),
                "listener_tls_profiles",
            )?,
            vhost_tls_overrides: usize_to_u32(
                std::iter::once(&config.default_vhost)
                    .chain(config.vhosts.iter())
                    .filter(|vhost| vhost.tls.is_some())
                    .count(),
                "vhost_tls_overrides",
            )?,
            default_certificate_bindings: default_certificates,
        },
    })
}

fn listener_model(
    listener_count: usize,
    first_listener_id: Option<&str>,
    first_listener_name: Option<&str>,
) -> &'static str {
    if listener_count == 1
        && first_listener_id == Some("default")
        && first_listener_name == Some("default")
    {
        "legacy"
    } else {
        "explicit"
    }
}

fn build_diff_response(
    left_label: String,
    left: &str,
    right_label: String,
    right: &str,
) -> ConfigDiffResponse {
    let lines = diff_lines(left, right);
    let changed = lines.iter().any(|line| !matches!(line.kind, ConfigDiffLineKind::Context));
    ConfigDiffResponse { left_label, right_label, changed, lines }
}

fn diff_lines(left: &str, right: &str) -> Vec<ConfigDiffLine> {
    let left_lines = left.lines().collect::<Vec<_>>();
    let right_lines = right.lines().collect::<Vec<_>>();
    let mut dp = vec![vec![0_usize; right_lines.len() + 1]; left_lines.len() + 1];

    for i in (0..left_lines.len()).rev() {
        for j in (0..right_lines.len()).rev() {
            dp[i][j] = if left_lines[i] == right_lines[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }

    let mut i = 0_usize;
    let mut j = 0_usize;
    let mut left_line = 1_u32;
    let mut right_line = 1_u32;
    let mut output = Vec::new();

    while i < left_lines.len() && j < right_lines.len() {
        if left_lines[i] == right_lines[j] {
            output.push(ConfigDiffLine {
                kind: ConfigDiffLineKind::Context,
                left_line_number: Some(left_line),
                right_line_number: Some(right_line),
                text: left_lines[i].to_string(),
            });
            i += 1;
            j += 1;
            left_line += 1;
            right_line += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            output.push(ConfigDiffLine {
                kind: ConfigDiffLineKind::Removed,
                left_line_number: Some(left_line),
                right_line_number: None,
                text: left_lines[i].to_string(),
            });
            i += 1;
            left_line += 1;
        } else {
            output.push(ConfigDiffLine {
                kind: ConfigDiffLineKind::Added,
                left_line_number: None,
                right_line_number: Some(right_line),
                text: right_lines[j].to_string(),
            });
            j += 1;
            right_line += 1;
        }
    }

    while i < left_lines.len() {
        output.push(ConfigDiffLine {
            kind: ConfigDiffLineKind::Removed,
            left_line_number: Some(left_line),
            right_line_number: None,
            text: left_lines[i].to_string(),
        });
        i += 1;
        left_line += 1;
    }

    while j < right_lines.len() {
        output.push(ConfigDiffLine {
            kind: ConfigDiffLineKind::Added,
            left_line_number: None,
            right_line_number: Some(right_line),
            text: right_lines[j].to_string(),
        });
        j += 1;
        right_line += 1;
    }

    output
}

fn usize_to_u32(value: usize, field: &str) -> ServiceResult<u32> {
    u32::try_from(value).map_err(|_| ServiceError::Internal(format!("{field} should fit into u32")))
}

fn unix_time_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{build_compile_summary, diff_lines};

    #[test]
    fn compile_summary_uses_config_semantics() {
        let config = rginx_config::load_and_compile_from_str(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n        worker_threads: Some(2),\n        accept_workers: Some(1),\n    ),\n    server: ServerConfig(\n        listen: \"127.0.0.1:18080\",\n        server_names: [\"localhost\"],\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n        ),\n    ],\n)\n",
            Path::new("inline.ron"),
        )
        .expect("config should compile");
        let summary = build_compile_summary(&config).expect("summary should be built");

        assert_eq!(summary.listener_count, 1);
        assert_eq!(summary.total_route_count, 1);
        assert_eq!(summary.default_server_names, vec!["localhost".to_string()]);
    }

    #[test]
    fn diff_lines_marks_added_and_removed_lines() {
        let diff = diff_lines("a\nb\nc\n", "a\nc\nd\n");

        assert!(diff.iter().any(|line| matches!(line.kind, super::ConfigDiffLineKind::Removed)));
        assert!(diff.iter().any(|line| matches!(line.kind, super::ConfigDiffLineKind::Added)));
    }
}
