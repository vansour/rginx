use axum::{
    extract::FromRequestParts,
    http::{HeaderMap, header, request::Parts},
};
use rginx_control_service::AuthenticatedNodeAgent;
use rginx_control_types::{AuthRole, AuthenticatedActor};

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Clone)]
pub struct ViewerGuard(pub AuthenticatedActor);

#[derive(Debug, Clone)]
pub struct OperatorGuard(pub AuthenticatedActor);

#[derive(Debug, Clone)]
pub struct ViewerTokenGuard {
    pub actor: AuthenticatedActor,
    pub token: String,
}

#[derive(Debug, Clone)]
pub struct BootstrapAgentGuard;

#[derive(Debug, Clone)]
pub struct BoundAgentGuard(pub AuthenticatedNodeAgent);

impl FromRequestParts<AppState> for ViewerGuard {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let (actor, _) = authenticate(parts, state).await?;
        ensure_admin(&actor)?;
        Ok(Self(actor))
    }
}

impl FromRequestParts<AppState> for OperatorGuard {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let (actor, _) = authenticate(parts, state).await?;
        ensure_admin(&actor)?;
        Ok(Self(actor))
    }
}

impl FromRequestParts<AppState> for ViewerTokenGuard {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let (actor, token) = authenticate(parts, state).await?;
        ensure_admin(&actor)?;
        Ok(Self { actor, token })
    }
}

impl FromRequestParts<AppState> for BootstrapAgentGuard {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer_token(&parts.headers)
            .ok_or_else(|| ApiError::unauthorized("missing bearer token"))?;
        if token == state.agent_shared_token() {
            Ok(Self)
        } else {
            Err(ApiError::unauthorized("invalid agent bearer token"))
        }
    }
}

impl FromRequestParts<AppState> for BoundAgentGuard {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer_token(&parts.headers)
            .ok_or_else(|| ApiError::unauthorized("missing bearer token"))?;
        let identity =
            state.services().auth().authenticate_node_agent_token(token).map_err(ApiError::from)?;
        Ok(Self(identity))
    }
}

async fn authenticate(
    parts: &Parts,
    state: &AppState,
) -> Result<(AuthenticatedActor, String), ApiError> {
    let token = bearer_token(&parts.headers)
        .ok_or_else(|| ApiError::unauthorized("missing bearer token"))?;
    let actor = state.services().auth().authenticate_token(token).await.map_err(ApiError::from)?;
    Ok((actor, token.to_string()))
}

fn ensure_admin(actor: &AuthenticatedActor) -> Result<(), ApiError> {
    if actor.user.roles.iter().copied().any(|role| role == AuthRole::SuperAdmin) {
        Ok(())
    } else {
        Err(ApiError::forbidden("administrator access is required"))
    }
}

pub fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let header_value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    header_value.strip_prefix("Bearer ").or_else(|| header_value.strip_prefix("bearer "))
}
