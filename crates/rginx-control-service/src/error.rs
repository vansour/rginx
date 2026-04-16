use thiserror::Error;

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("authentication required")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("dependency unavailable: {0}")]
    DependencyUnavailable(String),
    #[error("internal error: {0}")]
    Internal(String),
}

pub type ServiceResult<T> = Result<T, ServiceError>;
