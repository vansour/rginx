use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rginx_control_service::ServiceError;
use serde_json::json;

use crate::request_context::synthetic_request_id;

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    retryable: bool,
    request_id: Option<String>,
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "auth.unauthorized",
            message: message.into(),
            retryable: false,
            request_id: None,
        }
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "auth.forbidden",
            message: message.into(),
            retryable: false,
            request_id: None,
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "resource.not_found",
            message: message.into(),
            retryable: false,
            request_id: None,
        }
    }

    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }
}

impl From<ServiceError> for ApiError {
    fn from(error: ServiceError) -> Self {
        match error {
            ServiceError::Unauthorized => Self::unauthorized("authentication required"),
            ServiceError::Forbidden => Self::forbidden("forbidden"),
            ServiceError::InvalidCredentials => Self {
                status: StatusCode::UNAUTHORIZED,
                code: "auth.invalid_credentials",
                message: "invalid username or password".to_string(),
                retryable: false,
                request_id: None,
            },
            ServiceError::BadRequest(message) => Self {
                status: StatusCode::BAD_REQUEST,
                code: "request.invalid",
                message,
                retryable: false,
                request_id: None,
            },
            ServiceError::NotFound(message) => Self::not_found(message),
            ServiceError::Conflict(message) => Self {
                status: StatusCode::CONFLICT,
                code: "resource.conflict",
                message,
                retryable: false,
                request_id: None,
            },
            ServiceError::DependencyUnavailable(message) => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "dependency.unavailable",
                message,
                retryable: true,
                request_id: None,
            },
            ServiceError::Internal(message) => {
                tracing::error!(error = %message, "control-plane API internal error");
                Self {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    code: "internal.unexpected",
                    message: "unexpected control-plane error".to_string(),
                    retryable: false,
                    request_id: None,
                }
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(json!({
            "error": {
                "code": self.code,
                "message": self.message,
                "request_id": self.request_id.unwrap_or_else(synthetic_request_id),
                "retryable": self.retryable,
                "details": {},
            }
        }));

        (self.status, body).into_response()
    }
}
