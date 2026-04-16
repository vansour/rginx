use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthRole {
    Viewer,
    Operator,
    SuperAdmin,
}

impl AuthRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Operator => "operator",
            Self::SuperAdmin => "super_admin",
        }
    }

    pub fn grants(self, required: Self) -> bool {
        self >= required
    }
}

impl FromStr for AuthRole {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "viewer" => Ok(Self::Viewer),
            "operator" => Ok(Self::Operator),
            "super_admin" => Ok(Self::SuperAdmin),
            _ => Err(format!("unknown auth role `{value}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthUserSummary {
    pub user_id: String,
    pub username: String,
    pub display_name: String,
    pub active: bool,
    pub roles: Vec<AuthRole>,
    pub created_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthSessionSummary {
    pub session_id: String,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthenticatedActor {
    pub user: AuthUserSummary,
    pub session: AuthSessionSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthLoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthLoginResponse {
    pub token: String,
    pub actor: AuthenticatedActor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateLocalUserRequest {
    pub username: String,
    pub display_name: String,
    pub password: String,
    pub role: AuthRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateLocalUserResponse {
    pub user: AuthUserSummary,
}
