use axum::{
    extract::State,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use rginx_control_types::{
    DnsDeploymentStatus, DnsDeploymentSummary, DnsRuntimeQueryStat, DnsRuntimeStatus,
};

use crate::state::AppState;

pub async fn get_metrics(State(state): State<AppState>) -> Response {
    match state.services().metrics().render_prometheus_metrics().await {
        Ok(mut body) => {
            append_dns_runtime_metrics(&mut body, &state);
            if let Err(error) = append_dns_deployment_metrics(&mut body, &state).await {
                tracing::warn!(error = %error, "dns deployment metrics collection failed");
            }
            (
                StatusCode::OK,
                [(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
                )],
                body,
            )
                .into_response()
        }
        Err(error) => {
            tracing::warn!(error = %error, "metrics collection failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                [(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"))],
                "metrics collection failed".to_string(),
            )
                .into_response()
        }
    }
}

async fn append_dns_deployment_metrics(body: &mut String, state: &AppState) -> anyhow::Result<()> {
    let deployments = state.services().dns_deployments().list_deployments().await?;
    append_dns_deployment_metrics_from_deployments(body, &deployments);
    Ok(())
}

fn append_dns_deployment_metrics_from_deployments(
    body: &mut String,
    deployments: &[DnsDeploymentSummary],
) {
    if deployments.is_empty() {
        return;
    }

    body.push_str("# HELP rginx_control_dns_deployment_info DNS deployment metadata.\n");
    body.push_str("# TYPE rginx_control_dns_deployment_info gauge\n");
    body.push_str(
        "# HELP rginx_control_dns_deployment_targets DNS deployment target counts by state.\n",
    );
    body.push_str("# TYPE rginx_control_dns_deployment_targets gauge\n");
    body.push_str(
        "# HELP rginx_control_dns_deployments_active Active DNS deployments by cluster.\n",
    );
    body.push_str("# TYPE rginx_control_dns_deployments_active gauge\n");

    let mut active_per_cluster = std::collections::BTreeMap::<String, u64>::new();
    for deployment in deployments {
        if matches!(deployment.status, DnsDeploymentStatus::Running | DnsDeploymentStatus::Paused) {
            *active_per_cluster.entry(deployment.cluster_id.clone()).or_default() += 1;
        }
        append_metric(
            body,
            "rginx_control_dns_deployment_info",
            &[
                ("cluster_id", deployment.cluster_id.as_str()),
                ("deployment_id", deployment.deployment_id.as_str()),
                ("revision_id", deployment.revision_id.as_str()),
                ("version", deployment.revision_version_label.as_str()),
                ("status", deployment.status.as_str()),
                (
                    "promotes_cluster_runtime",
                    if deployment.promotes_cluster_runtime { "true" } else { "false" },
                ),
            ],
            1,
        );
        append_metric(
            body,
            "rginx_control_dns_deployment_targets",
            &[
                ("cluster_id", deployment.cluster_id.as_str()),
                ("deployment_id", deployment.deployment_id.as_str()),
                ("state", "pending"),
            ],
            u64::from(deployment.pending_nodes),
        );
        append_metric(
            body,
            "rginx_control_dns_deployment_targets",
            &[
                ("cluster_id", deployment.cluster_id.as_str()),
                ("deployment_id", deployment.deployment_id.as_str()),
                ("state", "active"),
            ],
            u64::from(deployment.active_nodes),
        );
        append_metric(
            body,
            "rginx_control_dns_deployment_targets",
            &[
                ("cluster_id", deployment.cluster_id.as_str()),
                ("deployment_id", deployment.deployment_id.as_str()),
                ("state", "succeeded"),
            ],
            u64::from(deployment.healthy_nodes),
        );
        append_metric(
            body,
            "rginx_control_dns_deployment_targets",
            &[
                ("cluster_id", deployment.cluster_id.as_str()),
                ("deployment_id", deployment.deployment_id.as_str()),
                ("state", "failed"),
            ],
            u64::from(deployment.failed_nodes),
        );
    }

    for (cluster_id, active_count) in active_per_cluster {
        append_metric(
            body,
            "rginx_control_dns_deployments_active",
            &[("cluster_id", cluster_id.as_str())],
            active_count,
        );
    }
}

fn append_dns_runtime_metrics(body: &mut String, state: &AppState) {
    let Some(runtime) = state.dns_runtime() else {
        return;
    };
    let status = runtime.runtime_status();
    append_dns_runtime_metrics_from_status(body, &status);
}

fn append_dns_runtime_metrics_from_status(body: &mut String, status: &[DnsRuntimeStatus]) {
    if status.is_empty() {
        return;
    }

    body.push_str("# HELP rginx_control_dns_queries_total Total authoritative DNS queries answered by rginx-web.\n");
    body.push_str("# TYPE rginx_control_dns_queries_total counter\n");
    body.push_str("# HELP rginx_control_dns_responses_total Total authoritative DNS responses by cluster and rcode.\n");
    body.push_str("# TYPE rginx_control_dns_responses_total counter\n");
    body.push_str("# HELP rginx_control_dns_published_revision_info Published DNS revision metadata loaded into the runtime cache.\n");
    body.push_str("# TYPE rginx_control_dns_published_revision_info gauge\n");
    body.push_str("# HELP rginx_control_dns_zone_total DNS zone count in the runtime cache.\n");
    body.push_str("# TYPE rginx_control_dns_zone_total gauge\n");
    body.push_str("# HELP rginx_control_dns_record_total DNS record count in the runtime cache.\n");
    body.push_str("# TYPE rginx_control_dns_record_total gauge\n");
    body.push_str("# HELP rginx_control_dns_hot_query_total Top authoritative DNS query counters by cluster and qname.\n");
    body.push_str("# TYPE rginx_control_dns_hot_query_total gauge\n");
    body.push_str("# HELP rginx_control_dns_error_query_total Top authoritative DNS error counters by cluster, qname and rcode.\n");
    body.push_str("# TYPE rginx_control_dns_error_query_total gauge\n");

    for item in status {
        append_metric(
            body,
            "rginx_control_dns_queries_total",
            &[("cluster_id", item.cluster_id.as_str())],
            item.query_total,
        );
        append_metric(
            body,
            "rginx_control_dns_responses_total",
            &[("cluster_id", item.cluster_id.as_str()), ("rcode", "noerror")],
            item.response_noerror_total,
        );
        append_metric(
            body,
            "rginx_control_dns_responses_total",
            &[("cluster_id", item.cluster_id.as_str()), ("rcode", "nxdomain")],
            item.response_nxdomain_total,
        );
        append_metric(
            body,
            "rginx_control_dns_responses_total",
            &[("cluster_id", item.cluster_id.as_str()), ("rcode", "servfail")],
            item.response_servfail_total,
        );
        append_metric(
            body,
            "rginx_control_dns_zone_total",
            &[("cluster_id", item.cluster_id.as_str())],
            u64::from(item.zone_count),
        );
        append_metric(
            body,
            "rginx_control_dns_record_total",
            &[("cluster_id", item.cluster_id.as_str())],
            u64::from(item.record_count),
        );
        append_metric(
            body,
            "rginx_control_dns_published_revision_info",
            &[
                ("cluster_id", item.cluster_id.as_str()),
                ("revision_id", item.published_revision_id.as_deref().unwrap_or("none")),
                ("version", item.published_revision_version.as_deref().unwrap_or("none")),
            ],
            u64::from(item.enabled),
        );
        for query in &item.hot_queries {
            append_dns_hot_query_metrics(body, &item.cluster_id, query);
        }
        for query in &item.error_queries {
            append_dns_error_query_metrics(body, &item.cluster_id, query);
        }
    }
}

fn append_dns_hot_query_metrics(body: &mut String, cluster_id: &str, query: &DnsRuntimeQueryStat) {
    let zone_name = query.zone_name.as_deref().unwrap_or("unmatched");
    append_metric(
        body,
        "rginx_control_dns_hot_query_total",
        &[
            ("cluster_id", cluster_id),
            ("zone_name", zone_name),
            ("qname", query.qname.as_str()),
            ("record_type", query.record_type.as_str()),
            ("kind", "queries"),
        ],
        query.query_total,
    );
    append_metric(
        body,
        "rginx_control_dns_hot_query_total",
        &[
            ("cluster_id", cluster_id),
            ("zone_name", zone_name),
            ("qname", query.qname.as_str()),
            ("record_type", query.record_type.as_str()),
            ("kind", "answers"),
        ],
        query.answer_total,
    );
}

fn append_dns_error_query_metrics(
    body: &mut String,
    cluster_id: &str,
    query: &DnsRuntimeQueryStat,
) {
    let zone_name = query.zone_name.as_deref().unwrap_or("unmatched");
    if query.response_nxdomain_total > 0 {
        append_metric(
            body,
            "rginx_control_dns_error_query_total",
            &[
                ("cluster_id", cluster_id),
                ("zone_name", zone_name),
                ("qname", query.qname.as_str()),
                ("record_type", query.record_type.as_str()),
                ("rcode", "nxdomain"),
            ],
            query.response_nxdomain_total,
        );
    }
    if query.response_servfail_total > 0 {
        append_metric(
            body,
            "rginx_control_dns_error_query_total",
            &[
                ("cluster_id", cluster_id),
                ("zone_name", zone_name),
                ("qname", query.qname.as_str()),
                ("record_type", query.record_type.as_str()),
                ("rcode", "servfail"),
            ],
            query.response_servfail_total,
        );
    }
}

fn append_metric(body: &mut String, name: &str, labels: &[(&str, &str)], value: u64) {
    body.push_str(name);
    if !labels.is_empty() {
        body.push('{');
        for (index, (label, label_value)) in labels.iter().enumerate() {
            if index > 0 {
                body.push(',');
            }
            body.push_str(label);
            body.push_str("=\"");
            body.push_str(label_value);
            body.push('"');
        }
        body.push('}');
    }
    body.push(' ');
    body.push_str(&value.to_string());
    body.push('\n');
}

#[cfg(test)]
mod tests {
    use rginx_control_types::{
        DnsDeploymentStatus, DnsDeploymentSummary, DnsRecordType, DnsRuntimeQueryStat,
        DnsRuntimeStatus,
    };

    use super::{
        append_dns_deployment_metrics_from_deployments, append_dns_runtime_metrics_from_status,
    };

    #[test]
    fn dns_deployment_metrics_skip_empty_input() {
        let mut body = String::new();
        append_dns_deployment_metrics_from_deployments(&mut body, &[]);
        assert!(body.is_empty());
    }

    #[test]
    fn dns_deployment_metrics_render_targets_and_active_cluster_counts() {
        let deployments = vec![
            sample_dns_deployment(
                "cluster-mainland",
                "dns-deploy-001",
                DnsDeploymentStatus::Running,
                1,
                2,
                3,
                0,
            ),
            sample_dns_deployment(
                "cluster-mainland",
                "dns-deploy-002",
                DnsDeploymentStatus::Paused,
                0,
                1,
                0,
                1,
            ),
            sample_dns_deployment(
                "cluster-backup",
                "dns-deploy-003",
                DnsDeploymentStatus::Succeeded,
                0,
                0,
                2,
                0,
            ),
        ];
        let mut body = String::new();

        append_dns_deployment_metrics_from_deployments(&mut body, &deployments);

        assert!(body.contains("# HELP rginx_control_dns_deployment_info DNS deployment metadata."));
        assert!(body.contains(
            "rginx_control_dns_deployment_info{cluster_id=\"cluster-mainland\",deployment_id=\"dns-deploy-001\",revision_id=\"dns_rev_local_0001\",version=\"dns-v1\",status=\"running\",promotes_cluster_runtime=\"true\"} 1"
        ));
        assert!(body.contains(
            "rginx_control_dns_deployment_targets{cluster_id=\"cluster-mainland\",deployment_id=\"dns-deploy-001\",state=\"active\"} 2"
        ));
        assert!(body.contains(
            "rginx_control_dns_deployment_targets{cluster_id=\"cluster-mainland\",deployment_id=\"dns-deploy-002\",state=\"failed\"} 1"
        ));
        assert!(
            body.contains(
                "rginx_control_dns_deployments_active{cluster_id=\"cluster-mainland\"} 2"
            )
        );
        assert!(
            !body.contains("rginx_control_dns_deployments_active{cluster_id=\"cluster-backup\"}")
        );
    }

    #[test]
    fn dns_runtime_metrics_render_hot_queries_and_error_breakdown() {
        let status = vec![DnsRuntimeStatus {
            enabled: true,
            cluster_id: "cluster-mainland".to_string(),
            udp_bind_addr: Some("127.0.0.1:5353".to_string()),
            tcp_bind_addr: Some("127.0.0.1:5354".to_string()),
            published_revision_id: Some("dns_rev_local_0001".to_string()),
            published_revision_version: Some("dns-v1".to_string()),
            zone_count: 1,
            record_count: 2,
            query_total: 3,
            response_noerror_total: 2,
            response_nxdomain_total: 1,
            response_servfail_total: 0,
            hot_queries: vec![DnsRuntimeQueryStat {
                zone_name: Some("example.com".to_string()),
                qname: "www.example.com".to_string(),
                record_type: DnsRecordType::A,
                query_total: 2,
                answer_total: 2,
                response_noerror_total: 2,
                response_nxdomain_total: 0,
                response_servfail_total: 0,
                last_query_at_unix_ms: 1_713_513_600_123,
            }],
            error_queries: vec![DnsRuntimeQueryStat {
                zone_name: Some("example.com".to_string()),
                qname: "missing.example.com".to_string(),
                record_type: DnsRecordType::A,
                query_total: 1,
                answer_total: 0,
                response_noerror_total: 0,
                response_nxdomain_total: 1,
                response_servfail_total: 0,
                last_query_at_unix_ms: 1_713_513_600_456,
            }],
        }];
        let mut body = String::new();

        append_dns_runtime_metrics_from_status(&mut body, &status);

        assert!(
            body.contains("rginx_control_dns_queries_total{cluster_id=\"cluster-mainland\"} 3")
        );
        assert!(body.contains(
            "rginx_control_dns_hot_query_total{cluster_id=\"cluster-mainland\",zone_name=\"example.com\",qname=\"www.example.com\",record_type=\"A\",kind=\"queries\"} 2"
        ));
        assert!(body.contains(
            "rginx_control_dns_hot_query_total{cluster_id=\"cluster-mainland\",zone_name=\"example.com\",qname=\"www.example.com\",record_type=\"A\",kind=\"answers\"} 2"
        ));
        assert!(body.contains(
            "rginx_control_dns_error_query_total{cluster_id=\"cluster-mainland\",zone_name=\"example.com\",qname=\"missing.example.com\",record_type=\"A\",rcode=\"nxdomain\"} 1"
        ));
    }

    fn sample_dns_deployment(
        cluster_id: &str,
        deployment_id: &str,
        status: DnsDeploymentStatus,
        pending_nodes: u32,
        active_nodes: u32,
        healthy_nodes: u32,
        failed_nodes: u32,
    ) -> DnsDeploymentSummary {
        DnsDeploymentSummary {
            deployment_id: deployment_id.to_string(),
            cluster_id: cluster_id.to_string(),
            revision_id: "dns_rev_local_0001".to_string(),
            revision_version_label: "dns-v1".to_string(),
            status,
            target_nodes: pending_nodes + active_nodes + healthy_nodes + failed_nodes,
            healthy_nodes,
            failed_nodes,
            active_nodes,
            pending_nodes,
            parallelism: 1,
            failure_threshold: 1,
            auto_rollback: false,
            promotes_cluster_runtime: true,
            created_by: "system".to_string(),
            rollback_of_deployment_id: None,
            rollback_revision_id: None,
            rolled_back_by_deployment_id: None,
            status_reason: None,
            created_at_unix_ms: 1_713_513_600_000,
            started_at_unix_ms: Some(1_713_513_600_000),
            finished_at_unix_ms: None,
        }
    }
}
