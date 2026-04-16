mod agent;
mod alerts;
mod audit;
mod auth;
mod dashboard;
mod deployments;
mod events;
mod health;
mod meta;
mod metrics;
mod nodes;
mod revisions;
mod users;

use axum::{
    Router,
    response::IntoResponse,
    routing::{any, get, post},
};

use crate::error::ApiError;

pub fn router() -> Router<crate::state::AppState> {
    Router::new()
        .route("/healthz", get(health::get_health))
        .route("/metrics", get(metrics::get_metrics))
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/logout", post(auth::logout))
        .route("/api/v1/auth/me", get(auth::get_me))
        .route("/api/v1/agent/register", post(agent::register))
        .route("/api/v1/agent/heartbeat", post(agent::heartbeat))
        .route("/api/v1/agent/tasks/poll", post(agent::poll_tasks))
        .route("/api/v1/agent/tasks/{task_id}/ack", post(agent::ack_task))
        .route("/api/v1/agent/tasks/{task_id}/complete", post(agent::complete_task))
        .route("/api/v1/agent/snapshots", post(agent::ingest_snapshot))
        .route("/api/v1/meta", get(meta::get_meta))
        .route("/api/v1/dashboard", get(dashboard::get_dashboard))
        .route("/api/v1/alerts", get(alerts::list_alerts))
        .route(
            "/api/v1/deployments",
            get(deployments::list_deployments).post(deployments::create_deployment),
        )
        .route("/api/v1/deployments/{deployment_id}", get(deployments::get_deployment))
        .route("/api/v1/deployments/{deployment_id}/pause", post(deployments::pause_deployment))
        .route("/api/v1/deployments/{deployment_id}/resume", post(deployments::resume_deployment))
        .route("/api/v1/revisions", get(revisions::list_revisions))
        .route(
            "/api/v1/revisions/drafts",
            get(revisions::list_drafts).post(revisions::create_draft),
        )
        .route("/api/v1/revisions/{revision_id}", get(revisions::get_revision))
        .route(
            "/api/v1/revisions/drafts/{draft_id}",
            get(revisions::get_draft).put(revisions::update_draft),
        )
        .route("/api/v1/revisions/drafts/{draft_id}/validate", post(revisions::validate_draft))
        .route("/api/v1/revisions/drafts/{draft_id}/diff", get(revisions::diff_draft))
        .route("/api/v1/revisions/drafts/{draft_id}/publish", post(revisions::publish_draft))
        .route("/api/v1/nodes", get(nodes::list_nodes))
        .route("/api/v1/nodes/{node_id}", get(nodes::get_node_detail))
        .route("/api/v1/events/session", post(events::create_session))
        .route("/api/v1/events", get(events::stream_events))
        .route("/api/v1/audit-logs", get(audit::list_audit_logs))
        .route("/api/v1/audit-logs/{audit_id}", get(audit::get_audit_log))
        .route("/api/v1/users", get(users::list_users).post(users::create_user))
        .route("/api", any(api_not_found))
        .route("/api/{*path}", any(api_not_found))
}

async fn api_not_found() -> impl IntoResponse {
    ApiError::not_found("route not found")
}
