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
    AuthRole, ControlPlaneDeploymentEvent, ControlPlaneNodeDetailEvent, ControlPlaneOverviewEvent,
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
