use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;

use rginx_control_types::{
    ConfigDiffResponse, ConfigDraftDetail, ConfigDraftSummary, ConfigRevisionDetail,
    ConfigRevisionListItem, CreateConfigDraftRequest, PublishConfigDraftRequest,
    PublishConfigDraftResponse, UpdateConfigDraftRequest,
};

use crate::auth::{OperatorGuard, ViewerGuard};
use crate::error::{ApiError, ApiResult};
use crate::request_context::RequestContext;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct DraftDiffQuery {
    pub target_revision_id: Option<String>,
}

pub async fn list_revisions(
    ViewerGuard(_actor): ViewerGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<ConfigRevisionListItem>>> {
    let revisions = state
        .services()
        .revisions()
        .list_revisions()
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(revisions))
}

pub async fn get_revision(
    ViewerGuard(_actor): ViewerGuard,
    request_context: RequestContext,
    Path(revision_id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<Json<ConfigRevisionDetail>> {
    let revision = state
        .services()
        .revisions()
        .get_revision(&revision_id)
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(revision))
}

pub async fn list_drafts(
    ViewerGuard(_actor): ViewerGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<ConfigDraftSummary>>> {
    let drafts = state
        .services()
        .revisions()
        .list_drafts()
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(drafts))
}

pub async fn get_draft(
    ViewerGuard(_actor): ViewerGuard,
    request_context: RequestContext,
    Path(draft_id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<Json<ConfigDraftDetail>> {
    let draft = state
        .services()
        .revisions()
        .get_draft(&draft_id)
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(draft))
}

pub async fn create_draft(
    OperatorGuard(actor): OperatorGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
    Json(request): Json<CreateConfigDraftRequest>,
) -> ApiResult<Json<ConfigDraftDetail>> {
    let draft = state
        .services()
        .revisions()
        .create_draft(
            &actor,
            request,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(draft))
}

pub async fn update_draft(
    OperatorGuard(actor): OperatorGuard,
    request_context: RequestContext,
    Path(draft_id): Path<String>,
    State(state): State<AppState>,
    Json(request): Json<UpdateConfigDraftRequest>,
) -> ApiResult<Json<ConfigDraftDetail>> {
    let draft = state
        .services()
        .revisions()
        .update_draft(
            &actor,
            &draft_id,
            request,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(draft))
}

pub async fn validate_draft(
    OperatorGuard(actor): OperatorGuard,
    request_context: RequestContext,
    Path(draft_id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<Json<ConfigDraftDetail>> {
    let draft = state
        .services()
        .revisions()
        .validate_draft(
            &actor,
            &draft_id,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(draft))
}

pub async fn diff_draft(
    ViewerGuard(_actor): ViewerGuard,
    request_context: RequestContext,
    Path(draft_id): Path<String>,
    Query(query): Query<DraftDiffQuery>,
    State(state): State<AppState>,
) -> ApiResult<Json<ConfigDiffResponse>> {
    let diff = state
        .services()
        .revisions()
        .diff_draft(&draft_id, query.target_revision_id.as_deref())
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(diff))
}

pub async fn publish_draft(
    OperatorGuard(actor): OperatorGuard,
    request_context: RequestContext,
    Path(draft_id): Path<String>,
    State(state): State<AppState>,
    Json(request): Json<PublishConfigDraftRequest>,
) -> ApiResult<Json<PublishConfigDraftResponse>> {
    let response = state
        .services()
        .revisions()
        .publish_draft(
            &actor,
            &draft_id,
            request,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(response))
}
