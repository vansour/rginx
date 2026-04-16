use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts},
};
use rginx_control_types::{AuthRole, AuthenticatedActor};

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Clone)]
pub struct ViewerGuard(pub AuthenticatedActor);

#[derive(Debug, Clone)]
pub struct OperatorGuard(pub AuthenticatedActor);

#[derive(Debug, Clone)]
pub struct SuperAdminGuard(pub AuthenticatedActor);

#[derive(Debug, Clone)]
pub struct AgentGuard;

impl FromRequestParts<AppState> for ViewerGuard {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let actor = authenticate(parts, state).await?;
        ensure_role(&actor, AuthRole::Viewer)?;
        Ok(Self(actor))
    }
}

impl FromRequestParts<AppState> for OperatorGuard {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let actor = authenticate(parts, state).await?;
        ensure_role(&actor, AuthRole::Operator)?;
        Ok(Self(actor))
    }
}

impl FromRequestParts<AppState> for SuperAdminGuard {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let actor = authenticate(parts, state).await?;
        ensure_role(&actor, AuthRole::SuperAdmin)?;
        Ok(Self(actor))
    }
}

impl FromRequestParts<AppState> for AgentGuard {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token =
            bearer_token(parts).ok_or_else(|| ApiError::unauthorized("missing bearer token"))?;
        if token == state.agent_shared_token() {
            Ok(Self)
        } else {
            Err(ApiError::unauthorized("invalid agent bearer token"))
        }
    }
}

async fn authenticate(parts: &Parts, state: &AppState) -> Result<AuthenticatedActor, ApiError> {
    let token =
        bearer_token(parts).ok_or_else(|| ApiError::unauthorized("missing bearer token"))?;
    state.services().auth().authenticate_token(token).await.map_err(ApiError::from)
}

fn ensure_role(actor: &AuthenticatedActor, required: AuthRole) -> Result<(), ApiError> {
    if actor.user.roles.iter().copied().any(|role| role.grants(required)) {
        Ok(())
    } else {
        Err(ApiError::forbidden(format!("required role `{}` is missing", required.as_str())))
    }
}

fn bearer_token(parts: &Parts) -> Option<&str> {
    let header_value = parts.headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    header_value.strip_prefix("Bearer ").or_else(|| header_value.strip_prefix("bearer "))
}
