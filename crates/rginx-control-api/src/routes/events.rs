use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    extract::{Query, State},
    response::sse::{Event, KeepAlive, Sse},
};
use futures_util::stream;
use rginx_control_types::{
    AuthRole, ControlPlaneDeploymentEvent, ControlPlaneNodeDetailEvent, ControlPlaneOverviewEvent,
};
use serde::Deserialize;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

static SSE_EVENT_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    pub access_token: Option<String>,
    pub node_id: Option<String>,
    pub deployment_id: Option<String>,
}

pub async fn stream_events(
    Query(query): Query<EventsQuery>,
    State(state): State<AppState>,
) -> ApiResult<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>> {
    let access_token = query
        .access_token
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ApiError::unauthorized("missing access_token query parameter"))?;
    let actor =
        state.services().auth().authenticate_token(&access_token).await.map_err(ApiError::from)?;
    if !actor.user.roles.iter().copied().any(|role| role.grants(AuthRole::Viewer)) {
        return Err(ApiError::forbidden("viewer role is required for event streams"));
    }

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

    let stream = stream::unfold(
        (state, query.node_id, query.deployment_id, true),
        |(state, node_id, deployment_id, immediate)| async move {
            if !immediate {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }

            let event = match deployment_id.as_deref() {
                Some(deployment_id) => build_deployment_event(&state, deployment_id).await,
                None => match node_id.as_deref() {
                    Some(node_id) => build_node_event(&state, node_id).await,
                    None => build_overview_event(&state).await,
                },
            };

            Some((Ok(event), (state, node_id, deployment_id, false)))
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

fn next_event_id(prefix: &str) -> String {
    let now = unix_time_ms(SystemTime::now());
    let sequence = SSE_EVENT_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{now}_{sequence}")
}

fn unix_time_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().min(u128::from(u64::MAX)) as u64
}
