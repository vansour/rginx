use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;
use ipnet::IpNet;
use serde_json::json;

use rginx_control_store::{
    ControlPlaneStore, DraftDnsValidationRecord, NewAuditLogEntry, NewDnsDraftRecord,
    NewDnsRevisionRecord, UpdateDnsDraftRecord,
};
use rginx_control_types::{
    AuthenticatedActor, CreateDnsDraftRequest, DnsAnswerTarget, DnsDiffResponse, DnsDraftDetail,
    DnsDraftSummary, DnsDraftValidationState, DnsPlan, DnsPublishedSnapshot, DnsRecordSet,
    DnsRecordType, DnsResolvedValue, DnsRevisionDetail, DnsRevisionListItem, DnsRuntimeStatus,
    DnsSimulationRequest, DnsSimulationResponse, DnsTargetKind, DnsValidationReport,
    PublishDnsDraftRequest, PublishDnsDraftResponse, UpdateDnsDraftRequest,
};

use crate::{ServiceError, ServiceResult};

static DNS_EVENT_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct DnsService {
    store: ControlPlaneStore,
}

impl DnsService {
    pub fn new(store: ControlPlaneStore) -> Self {
        Self { store }
    }

    pub async fn list_revisions(&self) -> ServiceResult<Vec<DnsRevisionListItem>> {
        self.store
            .dns_repository()
            .list_revisions()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))
    }

    pub async fn get_revision(&self, revision_id: &str) -> ServiceResult<DnsRevisionDetail> {
        self.store
            .dns_repository()
            .load_revision_detail(revision_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .ok_or_else(|| {
                ServiceError::NotFound(format!("dns revision `{revision_id}` was not found"))
            })
    }

    pub async fn list_drafts(&self) -> ServiceResult<Vec<DnsDraftSummary>> {
        self.store
            .dns_repository()
            .list_drafts()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))
    }

    pub async fn get_draft(&self, draft_id: &str) -> ServiceResult<DnsDraftDetail> {
        self.store
            .dns_repository()
            .load_draft_detail(draft_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .ok_or_else(|| ServiceError::NotFound(format!("dns draft `{draft_id}` was not found")))
    }

    pub async fn create_draft(
        &self,
        actor: &AuthenticatedActor,
        request: CreateDnsDraftRequest,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> ServiceResult<DnsDraftDetail> {
        let draft_id = self.generate_id("dns_draft");
        self.validate_draft_input(
            &request.cluster_id,
            &request.title,
            request.base_revision_id.as_deref(),
            &request.plan,
        )
        .await?;

        let now = Utc::now();
        let zone_count = request.plan.zones.len();
        self.store
            .dns_repository()
            .create_draft_with_audit(
                &NewDnsDraftRecord {
                    draft_id: draft_id.clone(),
                    cluster_id: request.cluster_id.clone(),
                    title: request.title.clone(),
                    summary: request.summary.clone(),
                    plan: request.plan,
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
                    action: "dns.draft_created".to_string(),
                    resource_type: "dns_draft".to_string(),
                    resource_id: draft_id,
                    result: "succeeded".to_string(),
                    details: json!({
                        "title": request.title,
                        "summary": request.summary,
                        "base_revision_id": request.base_revision_id,
                        "zone_count": zone_count,
                        "user_agent": user_agent,
                        "remote_addr": remote_addr,
                    }),
                    created_at: now,
                },
            )
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))
    }

    pub async fn update_draft(
        &self,
        actor: &AuthenticatedActor,
        draft_id: &str,
        request: UpdateDnsDraftRequest,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> ServiceResult<DnsDraftDetail> {
        let existing = self.get_draft(draft_id).await?;
        self.validate_draft_input(
            &existing.cluster_id,
            &request.title,
            request.base_revision_id.as_deref(),
            &request.plan,
        )
        .await?;

        let zone_count = request.plan.zones.len();
        self.store
            .dns_repository()
            .update_draft_with_audit(
                draft_id,
                &UpdateDnsDraftRecord {
                    title: request.title.clone(),
                    summary: request.summary.clone(),
                    plan: request.plan,
                    base_revision_id: request.base_revision_id.clone(),
                    updated_by: actor.user.username.clone(),
                    updated_at: Utc::now(),
                },
                &NewAuditLogEntry {
                    audit_id: self.generate_id("audit"),
                    request_id: request_id.to_string(),
                    cluster_id: Some(existing.cluster_id.clone()),
                    actor_id: actor.user.user_id.clone(),
                    action: "dns.draft_updated".to_string(),
                    resource_type: "dns_draft".to_string(),
                    resource_id: draft_id.to_string(),
                    result: "succeeded".to_string(),
                    details: json!({
                        "title": request.title,
                        "summary": request.summary,
                        "base_revision_id": request.base_revision_id,
                        "published_revision_id_cleared": existing.published_revision_id,
                        "zone_count": zone_count,
                        "user_agent": user_agent,
                        "remote_addr": remote_addr,
                    }),
                    created_at: Utc::now(),
                },
            )
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .ok_or_else(|| ServiceError::NotFound(format!("dns draft `{draft_id}` was not found")))
    }

    pub async fn validate_draft(
        &self,
        actor: &AuthenticatedActor,
        draft_id: &str,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> ServiceResult<DnsDraftDetail> {
        let draft = self.get_draft(draft_id).await?;
        let report = self.build_validation_report(&draft.plan).await?;
        self.persist_validation(
            actor,
            &draft,
            request_id,
            user_agent,
            remote_addr,
            "dns.draft_validated",
            report,
        )
        .await
    }

    pub async fn diff_draft(
        &self,
        draft_id: &str,
        target_revision_id: Option<&str>,
    ) -> ServiceResult<DnsDiffResponse> {
        let draft = self.get_draft(draft_id).await?;
        let draft_text = pretty_plan_json(&draft.plan)?;
        let target = match target_revision_id {
            Some(revision_id) => self.get_revision(revision_id).await?,
            None => match draft.base_revision_id.as_deref() {
                Some(revision_id) => self.get_revision(revision_id).await?,
                None => {
                    if let Some(revision) = self
                        .store
                        .dns_repository()
                        .load_latest_revision_for_cluster(&draft.cluster_id)
                        .await
                        .map_err(|error| ServiceError::Internal(error.to_string()))?
                    {
                        self.get_revision(&revision.revision_id).await?
                    } else {
                        return Ok(build_diff_response(
                            "empty revision".to_string(),
                            "",
                            format!("dns draft {}", draft.title),
                            &draft_text,
                        ));
                    }
                }
            },
        };

        Ok(build_diff_response(
            format!("dns revision {}", target.version_label),
            &pretty_plan_json(&target.plan)?,
            format!("dns draft {}", draft.title),
            &draft_text,
        ))
    }

    pub async fn publish_draft(
        &self,
        actor: &AuthenticatedActor,
        draft_id: &str,
        request: PublishDnsDraftRequest,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> ServiceResult<PublishDnsDraftResponse> {
        let draft = self.get_draft(draft_id).await?;
        if request.version_label.trim().is_empty() {
            return Err(ServiceError::BadRequest("version_label should not be empty".to_string()));
        }

        let report = self.build_validation_report(&draft.plan).await?;
        if !report.valid {
            let _ = self
                .persist_validation(
                    actor,
                    &draft,
                    request_id,
                    user_agent.clone(),
                    remote_addr.clone(),
                    "dns.publish_validation_failed",
                    report.clone(),
                )
                .await;
            return Err(ServiceError::BadRequest(
                "dns draft is invalid; validate and fix it before publishing".to_string(),
            ));
        }

        let now = Utc::now();
        let revision_id = self.generate_id("dns_rev");
        let published_summary = request.summary.unwrap_or_else(|| draft.summary.clone());
        let published = self
            .store
            .dns_repository()
            .publish_draft_with_audit(
                draft_id,
                &NewDnsRevisionRecord {
                    revision_id: revision_id.clone(),
                    cluster_id: draft.cluster_id.clone(),
                    version_label: request.version_label.clone(),
                    summary: published_summary.clone(),
                    plan: draft.plan.clone(),
                    validation: report.clone(),
                    created_by: actor.user.username.clone(),
                    created_at: now,
                    published_at: now,
                },
                &NewAuditLogEntry {
                    audit_id: self.generate_id("audit"),
                    request_id: request_id.to_string(),
                    cluster_id: Some(draft.cluster_id.clone()),
                    actor_id: actor.user.user_id.clone(),
                    action: "dns.published".to_string(),
                    resource_type: "dns_revision".to_string(),
                    resource_id: revision_id,
                    result: "succeeded".to_string(),
                    details: json!({
                        "draft_id": draft.draft_id,
                        "title": draft.title,
                        "version_label": request.version_label,
                        "summary": published_summary,
                        "base_revision_id": draft.base_revision_id,
                        "zone_count": draft.plan.zones.len(),
                        "user_agent": user_agent,
                        "remote_addr": remote_addr,
                    }),
                    created_at: now,
                },
            )
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .ok_or_else(|| {
                ServiceError::NotFound(format!("dns draft `{draft_id}` was not found"))
            })?;

        Ok(PublishDnsDraftResponse { draft: published.0, revision: published.1 })
    }

    pub async fn simulate_query(
        &self,
        request: DnsSimulationRequest,
    ) -> ServiceResult<DnsSimulationResponse> {
        let plan = match (request.draft_id.as_deref(), request.revision_id.as_deref()) {
            (Some(draft_id), _) => self.get_draft(draft_id).await?.plan,
            (None, Some(revision_id)) => self.get_revision(revision_id).await?.plan,
            (None, None) => {
                self.store
                    .dns_repository()
                    .load_published_revision_for_cluster(&request.cluster_id)
                    .await
                    .map_err(|error| ServiceError::Internal(error.to_string()))?
                    .ok_or_else(|| {
                        ServiceError::NotFound(format!(
                            "cluster `{}` does not have a published dns revision",
                            request.cluster_id
                        ))
                    })?
                    .plan
            }
        };
        let source_ip = request
            .source_ip
            .parse::<IpAddr>()
            .map_err(|error| ServiceError::BadRequest(format!("invalid source_ip: {error}")))?;
        evaluate_plan(
            &self.store,
            &request.cluster_id,
            &plan,
            &request.qname,
            request.record_type,
            source_ip,
        )
        .await
    }

    pub async fn runtime_status(&self) -> ServiceResult<Vec<DnsRuntimeStatus>> {
        self.store
            .dns_repository()
            .load_runtime_status()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))
    }

    pub async fn published_snapshot_for_node(
        &self,
        cluster_id: &str,
        node_id: &str,
    ) -> ServiceResult<Option<DnsPublishedSnapshot>> {
        let published = self
            .store
            .dns_deployment_repository()
            .load_effective_revision_for_node(cluster_id, node_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        let Some(published) = published else {
            return Ok(None);
        };

        let nodes = self
            .store
            .node_repository()
            .list_nodes()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        let resolved_upstreams = load_resolved_upstreams(&self.store, cluster_id).await?;

        Ok(Some(DnsPublishedSnapshot {
            cluster_id: published.cluster_id,
            revision_id: published.revision_id,
            version_label: published.version_label,
            plan: published.plan,
            nodes,
            resolved_upstreams,
        }))
    }

    async fn validate_draft_input(
        &self,
        cluster_id: &str,
        title: &str,
        base_revision_id: Option<&str>,
        plan: &DnsPlan,
    ) -> ServiceResult<()> {
        if cluster_id.trim().is_empty() {
            return Err(ServiceError::BadRequest("cluster_id should not be empty".to_string()));
        }
        if title.trim().is_empty() {
            return Err(ServiceError::BadRequest("title should not be empty".to_string()));
        }
        if plan.cluster_id != cluster_id {
            return Err(ServiceError::BadRequest(
                "plan.cluster_id must match request cluster_id".to_string(),
            ));
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

    async fn build_validation_report(&self, plan: &DnsPlan) -> ServiceResult<DnsValidationReport> {
        let issues = validate_dns_plan(&self.store, plan).await?;
        let record_count = plan.zones.iter().map(|zone| zone.records.len()).sum::<usize>();
        let target_count = plan
            .zones
            .iter()
            .flat_map(|zone| zone.records.iter())
            .map(|record| record.targets.len())
            .sum::<usize>();
        Ok(DnsValidationReport {
            valid: issues.is_empty(),
            validated_at_unix_ms: unix_time_ms(SystemTime::now()),
            issues,
            zone_count: usize_to_u32(plan.zones.len(), "zone_count")?,
            record_count: usize_to_u32(record_count, "record_count")?,
            target_count: usize_to_u32(target_count, "target_count")?,
        })
    }

    async fn persist_validation(
        &self,
        actor: &AuthenticatedActor,
        draft: &DnsDraftDetail,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
        action: &str,
        report: DnsValidationReport,
    ) -> ServiceResult<DnsDraftDetail> {
        self.store
            .dns_repository()
            .store_validation_with_audit(
                &draft.draft_id,
                &DraftDnsValidationRecord {
                    validation_state: if report.valid {
                        DnsDraftValidationState::Valid
                    } else {
                        DnsDraftValidationState::Invalid
                    },
                    validation_report: report.clone(),
                    updated_by: actor.user.username.clone(),
                    updated_at: Utc::now(),
                },
                &NewAuditLogEntry {
                    audit_id: self.generate_id("audit"),
                    request_id: request_id.to_string(),
                    cluster_id: Some(draft.cluster_id.clone()),
                    actor_id: actor.user.user_id.clone(),
                    action: action.to_string(),
                    resource_type: "dns_draft".to_string(),
                    resource_id: draft.draft_id.clone(),
                    result: if report.valid { "succeeded" } else { "failed" }.to_string(),
                    details: json!({
                        "title": draft.title,
                        "valid": report.valid,
                        "issues": report.issues,
                        "zone_count": report.zone_count,
                        "record_count": report.record_count,
                        "target_count": report.target_count,
                        "user_agent": user_agent,
                        "remote_addr": remote_addr,
                    }),
                    created_at: Utc::now(),
                },
            )
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .ok_or_else(|| {
                ServiceError::NotFound(format!("dns draft `{}` was not found", draft.draft_id))
            })
    }

    fn generate_id(&self, prefix: &str) -> String {
        let now = unix_time_ms(SystemTime::now());
        let sequence = DNS_EVENT_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{prefix}_{now}_{sequence}")
    }
}

async fn validate_dns_plan(
    store: &ControlPlaneStore,
    plan: &DnsPlan,
) -> ServiceResult<Vec<String>> {
    let mut issues = Vec::new();
    let mut zone_names = std::collections::HashSet::new();
    let nodes = store
        .node_repository()
        .list_nodes()
        .await
        .map_err(|error| ServiceError::Internal(error.to_string()))?;

    for zone in &plan.zones {
        if zone.zone_id.trim().is_empty() {
            issues.push("zone_id should not be empty".to_string());
        }
        if zone.zone_name.trim().is_empty() {
            issues.push(format!("zone `{}` zone_name should not be empty", zone.zone_id));
            continue;
        }
        let zone_name = normalize_name(&zone.zone_name);
        if !zone_names.insert(zone_name.clone()) {
            issues.push(format!("duplicate zone_name `{zone_name}`"));
        }

        let mut record_keys = std::collections::HashMap::<String, DnsRecordType>::new();
        let mut cname_names = std::collections::HashSet::new();
        let mut non_cname_names = std::collections::HashSet::new();
        for record in &zone.records {
            validate_record(plan, zone, record, &nodes, &mut issues).await;
            let fqdn = record_fqdn(zone, &record.name);
            let key = format!("{fqdn}:{}", record.record_type.as_str());
            if record_keys.insert(key.clone(), record.record_type).is_some() {
                issues.push(format!(
                    "duplicate record `{}` type `{}` in zone `{}`",
                    fqdn,
                    record.record_type.as_str(),
                    zone.zone_name
                ));
            }
            if record.record_type == DnsRecordType::Cname {
                cname_names.insert(fqdn.clone());
            } else {
                non_cname_names.insert(fqdn.clone());
            }
        }

        for fqdn in cname_names.intersection(&non_cname_names) {
            issues.push(format!(
                "record `{fqdn}` in zone `{}` cannot mix CNAME with other types",
                zone.zone_name
            ));
        }
    }

    Ok(issues)
}

async fn validate_record(
    plan: &DnsPlan,
    zone: &rginx_control_types::DnsZoneSpec,
    record: &DnsRecordSet,
    nodes: &[rginx_control_types::NodeSummary],
    issues: &mut Vec<String>,
) {
    if record.record_id.trim().is_empty() {
        issues.push(format!("zone `{}` contains a record with empty record_id", zone.zone_name));
    }
    if record.ttl_secs == 0 {
        issues.push(format!(
            "record `{}` in zone `{}` ttl_secs must be greater than 0",
            record.name, zone.zone_name
        ));
    }
    match record.record_type {
        DnsRecordType::A | DnsRecordType::Aaaa => {
            if record.targets.is_empty() && record.values.is_empty() {
                issues.push(format!(
                    "record `{}` in zone `{}` requires targets or values for `{}`",
                    record.name,
                    zone.zone_name,
                    record.record_type.as_str()
                ));
            }
            for value in &record.values {
                match value.parse::<IpAddr>() {
                    Ok(IpAddr::V4(_)) if record.record_type == DnsRecordType::A => {}
                    Ok(IpAddr::V6(_)) if record.record_type == DnsRecordType::Aaaa => {}
                    Ok(_) => issues.push(format!(
                        "record `{}` in zone `{}` has mismatched IP family `{value}` for `{}`",
                        record.name,
                        zone.zone_name,
                        record.record_type.as_str()
                    )),
                    Err(error) => issues.push(format!(
                        "record `{}` in zone `{}` has invalid IP value `{value}`: {error}",
                        record.name, zone.zone_name
                    )),
                }
            }
            for target in &record.targets {
                validate_target(plan, zone, record, target, nodes, issues);
            }
        }
        DnsRecordType::Cname => {
            if record.values.len() != 1 || !record.targets.is_empty() {
                issues.push(format!(
                    "CNAME record `{}` in zone `{}` requires exactly one value and no targets",
                    record.name, zone.zone_name
                ));
            }
        }
        DnsRecordType::Txt => {
            if record.values.is_empty() || !record.targets.is_empty() {
                issues.push(format!(
                    "TXT record `{}` in zone `{}` requires one or more values and no targets",
                    record.name, zone.zone_name
                ));
            }
        }
    }
}

fn validate_target(
    plan: &DnsPlan,
    zone: &rginx_control_types::DnsZoneSpec,
    record: &DnsRecordSet,
    target: &DnsAnswerTarget,
    nodes: &[rginx_control_types::NodeSummary],
    issues: &mut Vec<String>,
) {
    if target.target_id.trim().is_empty() {
        issues.push(format!(
            "record `{}` in zone `{}` contains target with empty target_id",
            record.name, zone.zone_name
        ));
    }
    if !target.enabled {
        return;
    }
    if target.weight == 0 {
        issues.push(format!(
            "target `{}` in record `{}` zone `{}` weight must be greater than 0",
            target.target_id, record.name, zone.zone_name
        ));
    }
    for cidr in &target.source_cidrs {
        if cidr.parse::<IpNet>().is_err() {
            issues.push(format!(
                "target `{}` in record `{}` zone `{}` has invalid source_cidr `{cidr}`",
                target.target_id, record.name, zone.zone_name
            ));
        }
    }
    match target.kind {
        DnsTargetKind::StaticIp => {
            if target.value.parse::<IpAddr>().is_err() {
                issues.push(format!(
                    "target `{}` in record `{}` zone `{}` has invalid static IP `{}`",
                    target.target_id, record.name, zone.zone_name, target.value
                ));
            }
        }
        DnsTargetKind::Cluster => {
            if target.value != plan.cluster_id {
                issues.push(format!(
                    "cluster target `{}` in record `{}` zone `{}` must currently reference plan.cluster_id `{}`",
                    target.target_id, record.name, zone.zone_name, plan.cluster_id
                ));
            }
        }
        DnsTargetKind::Node => {
            if !nodes.iter().any(|node| node.node_id == target.value) {
                issues.push(format!(
                    "target `{}` in record `{}` zone `{}` references unknown node `{}`",
                    target.target_id, record.name, zone.zone_name, target.value
                ));
            }
        }
        DnsTargetKind::Upstream => {
            if target.value.trim().is_empty() {
                issues.push(format!(
                    "target `{}` in record `{}` zone `{}` references empty upstream name",
                    target.target_id, record.name, zone.zone_name
                ));
            }
        }
    }
}

async fn evaluate_plan(
    store: &ControlPlaneStore,
    cluster_id: &str,
    plan: &DnsPlan,
    qname: &str,
    record_type: DnsRecordType,
    source_ip: IpAddr,
) -> ServiceResult<DnsSimulationResponse> {
    let qname = normalize_name(qname);
    let Some(zone) = best_matching_zone(&plan.zones, &qname) else {
        return Ok(DnsSimulationResponse {
            cluster_id: cluster_id.to_string(),
            qname,
            record_type,
            matched_zone: None,
            matched_record_id: None,
            ttl_secs: None,
            answers: Vec::new(),
            discarded: Vec::new(),
            issues: vec!["no matching zone".to_string()],
        });
    };
    let Some(record) = zone.records.iter().find(|record| {
        normalize_name(&record_fqdn(zone, &record.name)) == qname
            && record.record_type == record_type
    }) else {
        return Ok(DnsSimulationResponse {
            cluster_id: cluster_id.to_string(),
            qname,
            record_type,
            matched_zone: Some(zone.zone_name.clone()),
            matched_record_id: None,
            ttl_secs: None,
            answers: Vec::new(),
            discarded: Vec::new(),
            issues: vec!["no matching record".to_string()],
        });
    };

    let mut answers = Vec::new();
    let mut discarded = Vec::new();
    match record.record_type {
        DnsRecordType::A | DnsRecordType::Aaaa => {
            for value in &record.values {
                match value.parse::<IpAddr>() {
                    Ok(ip) if ip_matches_record_type(ip, record.record_type) => {
                        answers.push(DnsResolvedValue {
                            value: ip.to_string(),
                            target_kind: None,
                            target_id: None,
                            target_value: None,
                            weight: None,
                            source_cidrs: Vec::new(),
                            node_id: None,
                            cluster_id: None,
                            healthy: true,
                            reason: Some("static inline record value".to_string()),
                        })
                    }
                    Ok(ip) => discarded.push(DnsResolvedValue {
                        value: ip.to_string(),
                        target_kind: None,
                        target_id: None,
                        target_value: None,
                        weight: None,
                        source_cidrs: Vec::new(),
                        node_id: None,
                        cluster_id: None,
                        healthy: false,
                        reason: Some("record type does not match IP family".to_string()),
                    }),
                    Err(error) => discarded.push(DnsResolvedValue {
                        value: value.clone(),
                        target_kind: None,
                        target_id: None,
                        target_value: None,
                        weight: None,
                        source_cidrs: Vec::new(),
                        node_id: None,
                        cluster_id: None,
                        healthy: false,
                        reason: Some(format!("invalid IP value: {error}")),
                    }),
                }
            }
            for target in &record.targets {
                expand_target(
                    store,
                    cluster_id,
                    record.record_type,
                    source_ip,
                    target,
                    &mut answers,
                    &mut discarded,
                )
                .await?;
            }
        }
        DnsRecordType::Cname | DnsRecordType::Txt => {
            for value in &record.values {
                answers.push(DnsResolvedValue {
                    value: value.clone(),
                    target_kind: None,
                    target_id: None,
                    target_value: None,
                    weight: None,
                    source_cidrs: Vec::new(),
                    node_id: None,
                    cluster_id: Some(cluster_id.to_string()),
                    healthy: true,
                    reason: Some("literal record value".to_string()),
                });
            }
        }
    }

    Ok(DnsSimulationResponse {
        cluster_id: cluster_id.to_string(),
        qname,
        record_type,
        matched_zone: Some(zone.zone_name.clone()),
        matched_record_id: Some(record.record_id.clone()),
        ttl_secs: Some(record.ttl_secs),
        answers,
        discarded,
        issues: Vec::new(),
    })
}

async fn expand_target(
    store: &ControlPlaneStore,
    cluster_id: &str,
    record_type: DnsRecordType,
    source_ip: IpAddr,
    target: &DnsAnswerTarget,
    answers: &mut Vec<DnsResolvedValue>,
    discarded: &mut Vec<DnsResolvedValue>,
) -> ServiceResult<()> {
    if !target.enabled {
        discarded.push(discarded_target(target, None, "target is disabled"));
        return Ok(());
    }
    if !target.source_cidrs.is_empty() {
        let matched = target
            .source_cidrs
            .iter()
            .filter_map(|cidr| cidr.parse::<IpNet>().ok())
            .any(|cidr| cidr.contains(&source_ip));
        if !matched {
            discarded.push(discarded_target(target, None, "source_ip did not match source_cidrs"));
            return Ok(());
        }
    }

    match target.kind {
        DnsTargetKind::StaticIp => match target.value.parse::<IpAddr>() {
            Ok(ip) if ip_matches_record_type(ip, record_type) => {
                answers.push(resolved_target(
                    target,
                    ip.to_string(),
                    true,
                    None,
                    None,
                    Some("static target".to_string()),
                ));
            }
            Ok(ip) => discarded.push(discarded_target(
                target,
                Some(ip.to_string()),
                "static IP family does not match record type",
            )),
            Err(error) => discarded.push(discarded_target(
                target,
                None,
                &format!("invalid static IP: {error}"),
            )),
        },
        DnsTargetKind::Node => {
            let node = store
                .node_repository()
                .load_node_summary(&target.value)
                .await
                .map_err(|error| ServiceError::Internal(error.to_string()))?;
            let Some(node) = node else {
                discarded.push(discarded_target(target, None, "node was not found"));
                return Ok(());
            };
            let Some(ip) = advertise_ip(&node.advertise_addr) else {
                discarded.push(discarded_target(
                    target,
                    Some(node.advertise_addr),
                    "node advertise_addr does not contain a valid IP",
                ));
                return Ok(());
            };
            if !ip_matches_record_type(ip, record_type) {
                discarded.push(discarded_target(
                    target,
                    Some(ip.to_string()),
                    "node IP family does not match record type",
                ));
                return Ok(());
            }
            let healthy = matches!(
                node.state,
                rginx_control_types::NodeLifecycleState::Online
                    | rginx_control_types::NodeLifecycleState::Draining
            );
            let reason = Some(format!("node {} is {}", node.node_id, node.state.as_str()));
            let item = resolved_target(
                target,
                ip.to_string(),
                healthy,
                Some(node.node_id),
                Some(node.cluster_id),
                reason,
            );
            if healthy {
                answers.push(item);
            } else {
                discarded.push(item);
            }
        }
        DnsTargetKind::Cluster => {
            let nodes = store
                .node_repository()
                .list_nodes()
                .await
                .map_err(|error| ServiceError::Internal(error.to_string()))?;
            let cluster_nodes = nodes
                .into_iter()
                .filter(|node| node.cluster_id == target.value)
                .collect::<Vec<_>>();
            if cluster_nodes.is_empty() {
                discarded.push(discarded_target(
                    target,
                    None,
                    "cluster did not resolve to any nodes",
                ));
                return Ok(());
            }
            for node in cluster_nodes {
                let Some(ip) = advertise_ip(&node.advertise_addr) else {
                    discarded.push(discarded_target(
                        target,
                        Some(node.advertise_addr),
                        "cluster node advertise_addr does not contain a valid IP",
                    ));
                    continue;
                };
                if !ip_matches_record_type(ip, record_type) {
                    discarded.push(discarded_target(
                        target,
                        Some(ip.to_string()),
                        "cluster node IP family does not match record type",
                    ));
                    continue;
                }
                let healthy = matches!(
                    node.state,
                    rginx_control_types::NodeLifecycleState::Online
                        | rginx_control_types::NodeLifecycleState::Draining
                );
                let item = resolved_target(
                    target,
                    ip.to_string(),
                    healthy,
                    Some(node.node_id.clone()),
                    Some(node.cluster_id.clone()),
                    Some(format!("cluster expansion from node {}", node.node_id)),
                );
                if healthy {
                    answers.push(item);
                } else {
                    discarded.push(item);
                }
            }
        }
        DnsTargetKind::Upstream => {
            let Some(config_revision) = store
                .revision_repository()
                .load_latest_revision_for_cluster(cluster_id)
                .await
                .map_err(|error| ServiceError::Internal(error.to_string()))?
            else {
                discarded.push(discarded_target(
                    target,
                    None,
                    "cluster has no config revision to expand upstream target",
                ));
                return Ok(());
            };
            let detail = store
                .revision_repository()
                .load_revision_detail(&config_revision.revision_id)
                .await
                .map_err(|error| ServiceError::Internal(error.to_string()))?
                .ok_or_else(|| {
                    ServiceError::NotFound(format!(
                        "config revision `{}` was not found",
                        config_revision.revision_id
                    ))
                })?;
            match rginx_config::load_and_compile_from_str(
                &detail.config_text,
                Path::new(&detail.source_path),
            ) {
                Ok(compiled) => {
                    let Some(upstream) = compiled.upstreams.get(&target.value) else {
                        discarded.push(discarded_target(
                            target,
                            None,
                            "upstream target name was not found in latest config revision",
                        ));
                        return Ok(());
                    };
                    for peer in &upstream.peers {
                        let Ok(addrs) = peer.authority.to_socket_addrs() else {
                            discarded.push(discarded_target(
                                target,
                                Some(peer.authority.clone()),
                                "upstream authority could not be resolved",
                            ));
                            continue;
                        };
                        let mut matched = false;
                        for addr in addrs {
                            if ip_matches_record_type(addr.ip(), record_type) {
                                matched = true;
                                answers.push(resolved_target(
                                    target,
                                    addr.ip().to_string(),
                                    true,
                                    None,
                                    Some(cluster_id.to_string()),
                                    Some(format!("expanded from upstream peer {}", peer.url)),
                                ));
                            }
                        }
                        if !matched {
                            discarded.push(discarded_target(
                                target,
                                Some(peer.authority.clone()),
                                "upstream peer did not resolve to the requested record family",
                            ));
                        }
                    }
                }
                Err(error) => discarded.push(discarded_target(
                    target,
                    None,
                    &format!(
                        "failed to compile latest config revision for upstream expansion: {error}"
                    ),
                )),
            }
        }
    }

    Ok(())
}

fn resolved_target(
    target: &DnsAnswerTarget,
    value: String,
    healthy: bool,
    node_id: Option<String>,
    cluster_id: Option<String>,
    reason: Option<String>,
) -> DnsResolvedValue {
    DnsResolvedValue {
        value,
        target_kind: Some(target.kind),
        target_id: Some(target.target_id.clone()),
        target_value: Some(target.value.clone()),
        weight: Some(target.weight),
        source_cidrs: target.source_cidrs.clone(),
        node_id,
        cluster_id,
        healthy,
        reason,
    }
}

fn discarded_target(
    target: &DnsAnswerTarget,
    value: Option<String>,
    reason: &str,
) -> DnsResolvedValue {
    DnsResolvedValue {
        value: value.unwrap_or_else(|| target.value.clone()),
        target_kind: Some(target.kind),
        target_id: Some(target.target_id.clone()),
        target_value: Some(target.value.clone()),
        weight: Some(target.weight),
        source_cidrs: target.source_cidrs.clone(),
        node_id: None,
        cluster_id: None,
        healthy: false,
        reason: Some(reason.to_string()),
    }
}

fn advertise_ip(advertise_addr: &str) -> Option<IpAddr> {
    advertise_addr.to_socket_addrs().ok().and_then(|mut addrs| addrs.next()).map(|addr| addr.ip())
}

fn best_matching_zone<'a>(
    zones: &'a [rginx_control_types::DnsZoneSpec],
    qname: &str,
) -> Option<&'a rginx_control_types::DnsZoneSpec> {
    zones
        .iter()
        .filter(|zone| {
            let zone_name = normalize_name(&zone.zone_name);
            qname == zone_name || qname.ends_with(&format!(".{zone_name}"))
        })
        .max_by_key(|zone| zone.zone_name.len())
}

fn record_fqdn(zone: &rginx_control_types::DnsZoneSpec, record_name: &str) -> String {
    let record_name = record_name.trim();
    if record_name.is_empty() || record_name == "@" {
        normalize_name(&zone.zone_name)
    } else if record_name.ends_with('.') {
        normalize_name(record_name)
    } else {
        format!("{}.{}", normalize_name(record_name), normalize_name(&zone.zone_name))
    }
}

fn normalize_name(name: &str) -> String {
    name.trim().trim_end_matches('.').to_ascii_lowercase()
}

async fn load_resolved_upstreams(
    store: &ControlPlaneStore,
    cluster_id: &str,
) -> ServiceResult<BTreeMap<String, Vec<String>>> {
    let Some(revision) = store
        .revision_repository()
        .load_latest_revision_for_cluster(cluster_id)
        .await
        .map_err(|error| ServiceError::Internal(error.to_string()))?
    else {
        return Ok(BTreeMap::new());
    };
    let Some(detail) = store
        .revision_repository()
        .load_revision_detail(&revision.revision_id)
        .await
        .map_err(|error| ServiceError::Internal(error.to_string()))?
    else {
        return Ok(BTreeMap::new());
    };

    let compiled = match rginx_config::load_and_compile_from_str(
        &detail.config_text,
        Path::new(&detail.source_path),
    ) {
        Ok(compiled) => compiled,
        Err(error) => {
            tracing::warn!(
                cluster_id = %cluster_id,
                revision_id = %detail.revision_id,
                error = %error,
                "failed to compile latest config revision while preparing published dns snapshot"
            );
            return Ok(BTreeMap::new());
        }
    };

    let mut output = BTreeMap::new();
    for (upstream_name, upstream) in compiled.upstreams {
        let mut addrs = Vec::new();
        for peer in &upstream.peers {
            if let Ok(resolved) = peer.authority.to_socket_addrs() {
                addrs.extend(resolved.map(|addr: SocketAddr| addr.ip().to_string()));
            }
        }
        addrs.sort();
        addrs.dedup();
        output.insert(upstream_name, addrs);
    }
    Ok(output)
}

fn pretty_plan_json(plan: &DnsPlan) -> ServiceResult<String> {
    serde_json::to_string_pretty(plan).map_err(|error| {
        ServiceError::Internal(format!("failed to encode dns plan as json: {error}"))
    })
}

fn ip_matches_record_type(ip: IpAddr, record_type: DnsRecordType) -> bool {
    matches!(
        (ip, record_type),
        (IpAddr::V4(_), DnsRecordType::A) | (IpAddr::V6(_), DnsRecordType::Aaaa)
    )
}

fn build_diff_response(
    left_label: String,
    left: &str,
    right_label: String,
    right: &str,
) -> DnsDiffResponse {
    let lines = diff_lines(left, right);
    let changed = lines
        .iter()
        .any(|line| !matches!(line.kind, rginx_control_types::ConfigDiffLineKind::Context));
    DnsDiffResponse { left_label, right_label, changed, lines }
}

fn diff_lines(left: &str, right: &str) -> Vec<rginx_control_types::ConfigDiffLine> {
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
            output.push(rginx_control_types::ConfigDiffLine {
                kind: rginx_control_types::ConfigDiffLineKind::Context,
                left_line_number: Some(left_line),
                right_line_number: Some(right_line),
                text: left_lines[i].to_string(),
            });
            i += 1;
            j += 1;
            left_line += 1;
            right_line += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            output.push(rginx_control_types::ConfigDiffLine {
                kind: rginx_control_types::ConfigDiffLineKind::Removed,
                left_line_number: Some(left_line),
                right_line_number: None,
                text: left_lines[i].to_string(),
            });
            i += 1;
            left_line += 1;
        } else {
            output.push(rginx_control_types::ConfigDiffLine {
                kind: rginx_control_types::ConfigDiffLineKind::Added,
                left_line_number: None,
                right_line_number: Some(right_line),
                text: right_lines[j].to_string(),
            });
            j += 1;
            right_line += 1;
        }
    }

    while i < left_lines.len() {
        output.push(rginx_control_types::ConfigDiffLine {
            kind: rginx_control_types::ConfigDiffLineKind::Removed,
            left_line_number: Some(left_line),
            right_line_number: None,
            text: left_lines[i].to_string(),
        });
        i += 1;
        left_line += 1;
    }

    while j < right_lines.len() {
        output.push(rginx_control_types::ConfigDiffLine {
            kind: rginx_control_types::ConfigDiffLineKind::Added,
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
