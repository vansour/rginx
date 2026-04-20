use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::sse::{Event, KeepAlive, Sse},
    response::{IntoResponse, Response},
};
use futures_util::stream;
use rginx_control_types::{
    AuthRole, ControlPlaneDeploymentEvent, ControlPlaneDnsDeploymentEvent,
    ControlPlaneNodeDetailEvent, ControlPlaneOverviewEvent, DnsDeploymentDetail,
};
use serde::Deserialize;

use crate::auth::{ViewerTokenGuard, bearer_token};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

static SSE_EVENT_COUNTER: AtomicU64 = AtomicU64::new(1);
pub(crate) const EVENTS_SESSION_COOKIE_NAME: &str = "rginx_control_events_session";

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    pub node_id: Option<String>,
    pub deployment_id: Option<String>,
    pub dns_deployment_id: Option<String>,
}

pub async fn create_session(
    ViewerTokenGuard { actor, token }: ViewerTokenGuard,
) -> ApiResult<Response> {
    let max_age_secs = event_session_max_age_secs(actor.session.expires_at_unix_ms);
    let mut response = Json(serde_json::json!({ "status": "ok" })).into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&build_event_session_cookie_value(&token, max_age_secs))
            .expect("event session cookie should be valid"),
    );
    Ok(response)
}

pub async fn stream_events(
    Query(query): Query<EventsQuery>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> ApiResult<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>> {
    let access_token = event_session_token(&headers)
        .or_else(|| bearer_token(&headers).map(ToOwned::to_owned))
        .ok_or_else(|| ApiError::unauthorized("missing event session"))?;
    let actor =
        state.services().auth().authenticate_token(&access_token).await.map_err(ApiError::from)?;
    if !actor.user.roles.iter().copied().any(|role| role == AuthRole::SuperAdmin) {
        return Err(ApiError::forbidden("administrator access is required for event streams"));
    }

    validate_event_stream_scope(&query)?;

    if let Some(node_id) = query.node_id.as_deref() {
        state.services().nodes().get_node_detail(node_id).await.map_err(ApiError::from)?;
    }
    if let Some(deployment_id) = query.deployment_id.as_deref() {
        state
            .services()
            .deployments()
            .get_deployment_detail(deployment_id)
            .await
            .map_err(ApiError::from)?;
    }
    if let Some(deployment_id) = query.dns_deployment_id.as_deref() {
        state
            .services()
            .dns_deployments()
            .get_deployment_detail(deployment_id)
            .await
            .map_err(ApiError::from)?;
    }

    let stream = stream::unfold(
        (state, query.node_id, query.deployment_id, query.dns_deployment_id, true),
        |(state, node_id, deployment_id, dns_deployment_id, immediate)| async move {
            if !immediate {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }

            let event = match (deployment_id.as_deref(), dns_deployment_id.as_deref()) {
                (Some(deployment_id), None) => build_deployment_event(&state, deployment_id).await,
                (None, Some(dns_deployment_id)) => {
                    build_dns_deployment_event(&state, dns_deployment_id).await
                }
                (None, None) => match node_id.as_deref() {
                    Some(node_id) => build_node_event(&state, node_id).await,
                    None => build_overview_event(&state).await,
                },
                (Some(_), Some(_)) => error_event(
                    "invalid event stream query: deployment_id and dns_deployment_id conflict"
                        .to_string(),
                ),
            };

            Some((Ok(event), (state, node_id, deployment_id, dns_deployment_id, false)))
        },
    );

    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(10)).text("keep-alive")))
}

async fn build_overview_event(state: &AppState) -> Event {
    match (
        state.services().dashboard().get_dashboard_summary().await,
        state.services().nodes().list_nodes().await,
    ) {
        (Ok(dashboard), Ok(nodes)) => {
            let payload = ControlPlaneOverviewEvent {
                event_id: next_event_id("evt_overview"),
                emitted_at_unix_ms: unix_time_ms(SystemTime::now()),
                dashboard,
                nodes,
            };
            json_event("overview.tick", &payload)
        }
        (dashboard, nodes) => error_event(format!(
            "failed to build overview event: dashboard={:?}; nodes={:?}",
            dashboard.err(),
            nodes.err()
        )),
    }
}

async fn build_node_event(state: &AppState, node_id: &str) -> Event {
    match state.services().nodes().get_node_detail(node_id).await {
        Ok(detail) => {
            let payload = ControlPlaneNodeDetailEvent {
                event_id: next_event_id("evt_node"),
                emitted_at_unix_ms: unix_time_ms(SystemTime::now()),
                detail,
            };
            json_event("node.tick", &payload)
        }
        Err(error) => error_event(format!("failed to build node event for `{node_id}`: {error}")),
    }
}

async fn build_deployment_event(state: &AppState, deployment_id: &str) -> Event {
    match state.services().deployments().get_deployment_detail(deployment_id).await {
        Ok(detail) => {
            let payload = ControlPlaneDeploymentEvent {
                event_id: next_event_id("evt_deployment"),
                emitted_at_unix_ms: unix_time_ms(SystemTime::now()),
                detail,
            };
            json_event("deployment.tick", &payload)
        }
        Err(error) => {
            error_event(format!("failed to build deployment event for `{deployment_id}`: {error}"))
        }
    }
}

async fn build_dns_deployment_event(state: &AppState, deployment_id: &str) -> Event {
    match state.services().dns_deployments().get_deployment_detail(deployment_id).await {
        Ok(detail) => build_dns_deployment_tick_event(
            detail,
            next_event_id("evt_dns_deployment"),
            unix_time_ms(SystemTime::now()),
        ),
        Err(error) => error_event(format!(
            "failed to build dns deployment event for `{deployment_id}`: {error}"
        )),
    }
}

fn build_dns_deployment_tick_event(
    detail: DnsDeploymentDetail,
    event_id: String,
    emitted_at_unix_ms: u64,
) -> Event {
    json_event(
        "dns_deployment.tick",
        &ControlPlaneDnsDeploymentEvent { event_id, emitted_at_unix_ms, detail },
    )
}

fn json_event<T: serde::Serialize>(name: &str, payload: &T) -> Event {
    match serde_json::to_string(payload) {
        Ok(payload) => Event::default().event(name).data(payload),
        Err(error) => error_event(format!("failed to encode `{name}` payload: {error}")),
    }
}

fn error_event(message: String) -> Event {
    Event::default().event("stream.error").data(
        serde_json::json!({
            "event_id": next_event_id("evt_error"),
            "emitted_at_unix_ms": unix_time_ms(SystemTime::now()),
            "message": message,
        })
        .to_string(),
    )
}

fn validate_event_stream_scope(query: &EventsQuery) -> ApiResult<()> {
    if query.deployment_id.is_some() && query.dns_deployment_id.is_some() {
        return Err(ApiError::from(rginx_control_service::ServiceError::BadRequest(
            "only one of deployment_id or dns_deployment_id may be specified".to_string(),
        )));
    }
    Ok(())
}

fn next_event_id(prefix: &str) -> String {
    let now = unix_time_ms(SystemTime::now());
    let sequence = SSE_EVENT_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{now}_{sequence}")
}

fn unix_time_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().min(u128::from(u64::MAX)) as u64
}

pub(crate) fn build_event_session_cookie_value(token: &str, max_age_secs: u64) -> String {
    format!(
        "{EVENTS_SESSION_COOKIE_NAME}={token}; HttpOnly; Path=/api/v1/events; SameSite=Strict; Max-Age={max_age_secs}"
    )
}

pub(crate) fn clear_event_session_cookie_value() -> String {
    format!(
        "{EVENTS_SESSION_COOKIE_NAME}=; HttpOnly; Path=/api/v1/events; SameSite=Strict; Max-Age=0"
    )
}

fn event_session_max_age_secs(expires_at_unix_ms: u64) -> u64 {
    let now = unix_time_ms(SystemTime::now());
    let remaining_ms = expires_at_unix_ms.saturating_sub(now);
    remaining_ms.saturating_add(999).checked_div(1000).unwrap_or_default().max(1)
}

fn event_session_token(headers: &HeaderMap) -> Option<String> {
    headers.get(header::COOKIE)?.to_str().ok()?.split(';').find_map(|segment| {
        let (name, value) = segment.trim().split_once('=')?;
        (name == EVENTS_SESSION_COOKIE_NAME).then(|| value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use axum::{
        http::StatusCode,
        response::{
            IntoResponse,
            sse::{Event, Sse},
        },
    };
    use futures_util::stream;
    use rginx_control_types::{
        AuditLogSummary, DnsDeploymentDetail, DnsDeploymentStatus, DnsDeploymentSummary,
        DnsDeploymentTargetState, DnsDeploymentTargetSummary, DnsRevisionListItem,
        NodeLifecycleState,
    };

    use super::{EventsQuery, build_dns_deployment_tick_event, validate_event_stream_scope};

    #[tokio::test]
    async fn dns_deployment_tick_event_serializes_expected_payload() {
        let event = build_dns_deployment_tick_event(
            sample_dns_deployment_detail(),
            "evt_dns_deployment_test".to_string(),
            1_713_513_600_123,
        );
        let response =
            Sse::new(stream::once(async move { Ok::<Event, Infallible>(event) })).into_response();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("sse body should encode");
        let body = String::from_utf8(body.to_vec()).expect("sse body should be utf-8");

        assert!(body.contains("event: dns_deployment.tick"));
        assert!(body.contains("\"event_id\":\"evt_dns_deployment_test\""));
        assert!(body.contains("\"deployment_id\":\"dns-deploy-test-01\""));
        assert!(body.contains("\"revision_id\":\"dns_rev_test_01\""));
    }

    #[tokio::test]
    async fn conflicting_deployment_filters_are_rejected() {
        let error = validate_event_stream_scope(&EventsQuery {
            node_id: None,
            deployment_id: Some("deploy-01".to_string()),
            dns_deployment_id: Some("dns-deploy-01".to_string()),
        })
        .expect_err("conflicting scopes should be rejected");
        let response = error.into_response();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("error body should encode");
        let body = String::from_utf8(body.to_vec()).expect("error body should be utf-8");

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("only one of deployment_id or dns_deployment_id may be specified"));
    }

    fn sample_dns_deployment_detail() -> DnsDeploymentDetail {
        DnsDeploymentDetail {
            deployment: DnsDeploymentSummary {
                deployment_id: "dns-deploy-test-01".to_string(),
                cluster_id: "cluster-mainland".to_string(),
                revision_id: "dns_rev_test_01".to_string(),
                revision_version_label: "dns-v1".to_string(),
                status: DnsDeploymentStatus::Running,
                target_nodes: 1,
                healthy_nodes: 0,
                failed_nodes: 0,
                active_nodes: 1,
                pending_nodes: 0,
                parallelism: 1,
                failure_threshold: 1,
                auto_rollback: false,
                promotes_cluster_runtime: true,
                created_by: "admin".to_string(),
                rollback_of_deployment_id: None,
                rollback_revision_id: None,
                rolled_back_by_deployment_id: None,
                status_reason: Some("waiting for canary".to_string()),
                created_at_unix_ms: 1_713_513_600_000,
                started_at_unix_ms: Some(1_713_513_600_010),
                finished_at_unix_ms: None,
            },
            revision: DnsRevisionListItem {
                revision_id: "dns_rev_test_01".to_string(),
                cluster_id: "cluster-mainland".to_string(),
                version_label: "dns-v1".to_string(),
                summary: "smart dns revision".to_string(),
                created_by: "admin".to_string(),
                created_at_unix_ms: 1_713_513_500_000,
                published_at_unix_ms: Some(1_713_513_550_000),
            },
            rollback_revision: None,
            targets: vec![DnsDeploymentTargetSummary {
                target_id: "dns-target-01".to_string(),
                deployment_id: "dns-deploy-test-01".to_string(),
                node_id: "edge-hz-01".to_string(),
                advertise_addr: "10.0.0.11:8443".to_string(),
                node_state: NodeLifecycleState::Online,
                desired_revision_id: "dns_rev_test_01".to_string(),
                state: DnsDeploymentTargetState::Active,
                batch_index: 0,
                last_error: None,
                assigned_at_unix_ms: Some(1_713_513_600_020),
                confirmed_at_unix_ms: None,
                failed_at_unix_ms: None,
            }],
            recent_events: vec![AuditLogSummary {
                audit_id: "audit_dns_01".to_string(),
                request_id: "req_dns_01".to_string(),
                cluster_id: Some("cluster-mainland".to_string()),
                actor_id: "admin".to_string(),
                action: "dns.deployment.created".to_string(),
                resource_type: "dns_deployment".to_string(),
                resource_id: "dns-deploy-test-01".to_string(),
                result: "running".to_string(),
                created_at_unix_ms: 1_713_513_600_030,
            }],
        }
    }
}
