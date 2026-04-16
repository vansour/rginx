use std::num::NonZeroU32;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use ring::{
    hmac, pbkdf2,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use rginx_control_store::{
    ControlPlaneStore, NewAuditLogEntry, NewAuthSession, NewLocalUserRecord, StoredPasswordUser,
};
use rginx_control_types::{
    AuthLoginRequest, AuthLoginResponse, AuthUserSummary, AuthenticatedActor,
    CreateLocalUserRequest, CreateLocalUserResponse,
};

use crate::{ControlPlaneAuthConfig, ServiceError, ServiceResult};

const PASSWORD_SCHEME: &str = "pbkdf2_sha256";
const PASSWORD_ITERATIONS: u32 = 100_000;
const PASSWORD_HASH_LEN: usize = 32;
const PASSWORD_SALT_LEN: usize = 16;
const SESSION_TOKEN_LEN: usize = 32;
const ID_TOKEN_LEN: usize = 12;
const NODE_AGENT_TOKEN_SCOPE: &str = "node_agent_v1";
const NODE_AGENT_TOKEN_TTL_SECS: i64 = 3600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedNodeAgent {
    pub node_id: String,
    pub cluster_id: String,
    pub expires_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedNodeAgentToken {
    pub token: String,
    pub expires_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NodeAgentTokenClaims {
    node_id: String,
    cluster_id: String,
    expires_at_unix_ms: u64,
}

#[derive(Debug, Clone)]
pub struct AuthService {
    store: ControlPlaneStore,
    auth_config: Option<ControlPlaneAuthConfig>,
}

impl AuthService {
    pub fn new(store: ControlPlaneStore, auth_config: Option<ControlPlaneAuthConfig>) -> Self {
        Self { store, auth_config }
    }

    pub async fn authenticate_token(&self, token: &str) -> ServiceResult<AuthenticatedActor> {
        let token = token.trim();
        if token.is_empty() {
            return Err(ServiceError::Unauthorized);
        }

        let session_hash = self.hash_session_token(token)?;
        self.store
            .auth_repository()
            .load_actor_by_session_hash(&session_hash)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .ok_or(ServiceError::Unauthorized)
    }

    pub async fn login(
        &self,
        request: AuthLoginRequest,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> ServiceResult<AuthLoginResponse> {
        if request.username.trim().is_empty() || request.password.is_empty() {
            return Err(ServiceError::BadRequest(
                "username and password should not be empty".to_string(),
            ));
        }

        let maybe_user = self
            .store
            .auth_repository()
            .find_user_credentials_by_username(request.username.trim())
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;

        let stored_user = match maybe_user {
            Some(user) if user.user.active => user,
            _ => {
                self.record_failed_login_best_effort(
                    request_id,
                    request.username.trim(),
                    user_agent,
                    remote_addr,
                    "unknown user or inactive account",
                )
                .await;
                return Err(ServiceError::InvalidCredentials);
            }
        };

        if !self.verify_password(&stored_user, &request.password)? {
            self.record_failed_login_best_effort(
                request_id,
                &stored_user.user.username,
                user_agent,
                remote_addr,
                "password mismatch",
            )
            .await;
            return Err(ServiceError::InvalidCredentials);
        }

        let issued_at = Utc::now();
        let expires_at = issued_at
            + Duration::from_std(self.auth_config()?.session_ttl)
                .map_err(|error| ServiceError::Internal(error.to_string()))?;
        let token = self.generate_random_token(SESSION_TOKEN_LEN)?;
        let session_id = self.generate_id("sess")?;
        let session_hash = self.hash_session_token(&token)?;

        let session = NewAuthSession {
            session_id: session_id.clone(),
            user_id: stored_user.user.user_id.clone(),
            session_hash,
            issued_at,
            expires_at,
            user_agent: user_agent.clone(),
            remote_addr: remote_addr.clone(),
        };
        let audit = NewAuditLogEntry {
            audit_id: self.generate_id("audit")?,
            request_id: request_id.to_string(),
            cluster_id: None,
            actor_id: stored_user.user.user_id.clone(),
            action: "auth.login".to_string(),
            resource_type: "session".to_string(),
            resource_id: session_id,
            result: "succeeded".to_string(),
            details: json!({
                    "username": stored_user.user.username.clone(),
                    "roles": stored_user.user.roles.iter().map(|role| role.as_str()).collect::<Vec<_>>(),
                    "user_agent": user_agent,
                    "remote_addr": remote_addr,
            }),
            created_at: issued_at,
        };
        let session_summary = self
            .store
            .auth_repository()
            .create_session_with_audit(&session, &audit)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;

        Ok(AuthLoginResponse {
            token,
            actor: AuthenticatedActor { user: stored_user.user, session: session_summary },
        })
    }

    pub async fn logout(
        &self,
        actor: &AuthenticatedActor,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> ServiceResult<()> {
        let revoked = self
            .store
            .auth_repository()
            .revoke_session_with_audit(
                &actor.session.session_id,
                &NewAuditLogEntry {
                    audit_id: self.generate_id("audit")?,
                    request_id: request_id.to_string(),
                    cluster_id: None,
                    actor_id: actor.user.user_id.clone(),
                    action: "auth.logout".to_string(),
                    resource_type: "session".to_string(),
                    resource_id: actor.session.session_id.clone(),
                    result: "succeeded".to_string(),
                    details: json!({
                        "username": actor.user.username.clone(),
                        "user_agent": user_agent,
                        "remote_addr": remote_addr,
                    }),
                    created_at: Utc::now(),
                },
            )
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;

        if !revoked {
            return Err(ServiceError::Conflict("session already revoked or not found".to_string()));
        }

        Ok(())
    }

    pub fn current_actor(&self, actor: &AuthenticatedActor) -> AuthenticatedActor {
        actor.clone()
    }

    pub async fn list_users(&self) -> ServiceResult<Vec<AuthUserSummary>> {
        self.store
            .auth_repository()
            .list_users()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))
    }

    pub async fn create_local_user(
        &self,
        actor: &AuthenticatedActor,
        request: CreateLocalUserRequest,
        request_id: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
    ) -> ServiceResult<CreateLocalUserResponse> {
        if request.username.trim().is_empty() {
            return Err(ServiceError::BadRequest("username should not be empty".to_string()));
        }
        if request.display_name.trim().is_empty() {
            return Err(ServiceError::BadRequest("display_name should not be empty".to_string()));
        }
        if request.password.len() < 10 {
            return Err(ServiceError::BadRequest(
                "password should be at least 10 characters".to_string(),
            ));
        }

        if self
            .store
            .auth_repository()
            .find_user_credentials_by_username(request.username.trim())
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .is_some()
        {
            return Err(ServiceError::Conflict(format!(
                "user `{}` already exists",
                request.username.trim()
            )));
        }

        let created_at = Utc::now();
        let new_user = NewLocalUserRecord {
            user_id: self.generate_id("usr")?,
            username: request.username.trim().to_string(),
            display_name: request.display_name.trim().to_string(),
            password_hash: self.hash_password(&request.password)?,
            role: request.role,
        };
        let user = self
            .store
            .auth_repository()
            .create_local_user_with_audit(
                &new_user,
                &NewAuditLogEntry {
                    audit_id: self.generate_id("audit")?,
                    request_id: request_id.to_string(),
                    cluster_id: None,
                    actor_id: actor.user.user_id.clone(),
                    action: "auth.user_created".to_string(),
                    resource_type: "user".to_string(),
                    resource_id: new_user.user_id.clone(),
                    result: "succeeded".to_string(),
                    details: json!({
                        "username": new_user.username.clone(),
                        "display_name": new_user.display_name.clone(),
                        "role": new_user.role.as_str(),
                        "created_by": actor.user.username.clone(),
                        "user_agent": user_agent,
                        "remote_addr": remote_addr,
                    }),
                    created_at,
                },
            )
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;

        Ok(CreateLocalUserResponse { user })
    }

    pub fn mint_node_agent_token(
        &self,
        node_id: &str,
        cluster_id: &str,
    ) -> ServiceResult<IssuedNodeAgentToken> {
        if node_id.trim().is_empty() || cluster_id.trim().is_empty() {
            return Err(ServiceError::BadRequest(
                "node_id and cluster_id should not be empty".to_string(),
            ));
        }

        let expires_at = Utc::now() + Duration::seconds(NODE_AGENT_TOKEN_TTL_SECS);
        let expires_at_unix_ms = u64::try_from(expires_at.timestamp_millis()).unwrap_or(u64::MAX);
        let claims = NodeAgentTokenClaims {
            node_id: node_id.trim().to_string(),
            cluster_id: cluster_id.trim().to_string(),
            expires_at_unix_ms,
        };
        let token = self.sign_claims(NODE_AGENT_TOKEN_SCOPE, &claims)?;

        Ok(IssuedNodeAgentToken { token, expires_at_unix_ms })
    }

    pub fn authenticate_node_agent_token(
        &self,
        token: &str,
    ) -> ServiceResult<AuthenticatedNodeAgent> {
        let claims: NodeAgentTokenClaims = self.verify_claims(NODE_AGENT_TOKEN_SCOPE, token)?;
        if claims.node_id.trim().is_empty() || claims.cluster_id.trim().is_empty() {
            return Err(ServiceError::Unauthorized);
        }
        if claims.expires_at_unix_ms <= unix_time_ms(SystemTime::now()) {
            return Err(ServiceError::Unauthorized);
        }

        Ok(AuthenticatedNodeAgent {
            node_id: claims.node_id,
            cluster_id: claims.cluster_id,
            expires_at_unix_ms: claims.expires_at_unix_ms,
        })
    }

    fn auth_config(&self) -> ServiceResult<&ControlPlaneAuthConfig> {
        self.auth_config
            .as_ref()
            .ok_or_else(|| ServiceError::Internal("auth service is not configured".to_string()))
    }

    fn hash_password(&self, password: &str) -> ServiceResult<String> {
        let mut salt = [0_u8; PASSWORD_SALT_LEN];
        SystemRandom::new()
            .fill(&mut salt)
            .map_err(|_| ServiceError::Internal("failed to generate password salt".to_string()))?;

        let mut output = [0_u8; PASSWORD_HASH_LEN];
        pbkdf2::derive(
            pbkdf2::PBKDF2_HMAC_SHA256,
            NonZeroU32::new(PASSWORD_ITERATIONS).expect("non-zero iterations"),
            &salt,
            password.as_bytes(),
            &mut output,
        );

        Ok(format!(
            "{PASSWORD_SCHEME}${PASSWORD_ITERATIONS}${}${}",
            URL_SAFE_NO_PAD.encode(salt),
            URL_SAFE_NO_PAD.encode(output)
        ))
    }

    fn verify_password(
        &self,
        stored_user: &StoredPasswordUser,
        password: &str,
    ) -> ServiceResult<bool> {
        let mut parts = stored_user.password_hash.split('$');
        let scheme = parts.next().ok_or_else(|| {
            ServiceError::Internal("stored password hash is malformed".to_string())
        })?;
        let iterations = parts.next().ok_or_else(|| {
            ServiceError::Internal("stored password hash is malformed".to_string())
        })?;
        let salt = parts.next().ok_or_else(|| {
            ServiceError::Internal("stored password hash is malformed".to_string())
        })?;
        let expected = parts.next().ok_or_else(|| {
            ServiceError::Internal("stored password hash is malformed".to_string())
        })?;

        if scheme != PASSWORD_SCHEME {
            return Err(ServiceError::Internal(format!("unsupported password scheme `{scheme}`")));
        }

        let iterations = iterations
            .parse::<u32>()
            .context("stored password hash has invalid iteration count")
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        let iterations = NonZeroU32::new(iterations).ok_or_else(|| {
            ServiceError::Internal("iteration count should be non-zero".to_string())
        })?;
        let salt = URL_SAFE_NO_PAD
            .decode(salt)
            .context("stored password hash has invalid salt encoding")
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        let expected = URL_SAFE_NO_PAD
            .decode(expected)
            .context("stored password hash has invalid hash encoding")
            .map_err(|error| ServiceError::Internal(error.to_string()))?;

        Ok(pbkdf2::verify(
            pbkdf2::PBKDF2_HMAC_SHA256,
            iterations,
            &salt,
            password.as_bytes(),
            &expected,
        )
        .is_ok())
    }

    fn hash_session_token(&self, token: &str) -> ServiceResult<String> {
        let mut hasher = Sha256::new();
        hasher.update(self.auth_config()?.session_secret.as_bytes());
        hasher.update(b":");
        hasher.update(token.as_bytes());
        Ok(URL_SAFE_NO_PAD.encode(hasher.finalize()))
    }

    fn generate_random_token(&self, len: usize) -> ServiceResult<String> {
        let mut bytes = vec![0_u8; len];
        SystemRandom::new().fill(&mut bytes).map_err(|_| {
            ServiceError::Internal("failed to generate secure random token".to_string())
        })?;
        Ok(URL_SAFE_NO_PAD.encode(bytes))
    }

    fn sign_claims<T: Serialize>(&self, scope: &str, claims: &T) -> ServiceResult<String> {
        let payload = serde_json::to_vec(claims)
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        let key = hmac::Key::new(hmac::HMAC_SHA256, self.auth_config()?.session_secret.as_bytes());
        let mut signing_input = Vec::with_capacity(scope.len() + 1 + payload.len());
        signing_input.extend_from_slice(scope.as_bytes());
        signing_input.push(b':');
        signing_input.extend_from_slice(&payload);
        let signature = hmac::sign(&key, &signing_input);

        Ok(format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(payload),
            URL_SAFE_NO_PAD.encode(signature.as_ref())
        ))
    }

    fn verify_claims<T: for<'de> Deserialize<'de>>(
        &self,
        scope: &str,
        token: &str,
    ) -> ServiceResult<T> {
        let (encoded_payload, encoded_signature) =
            token.split_once('.').ok_or(ServiceError::Unauthorized)?;
        let payload =
            URL_SAFE_NO_PAD.decode(encoded_payload).map_err(|_| ServiceError::Unauthorized)?;
        let signature =
            URL_SAFE_NO_PAD.decode(encoded_signature).map_err(|_| ServiceError::Unauthorized)?;

        let key = hmac::Key::new(hmac::HMAC_SHA256, self.auth_config()?.session_secret.as_bytes());
        let mut signing_input = Vec::with_capacity(scope.len() + 1 + payload.len());
        signing_input.extend_from_slice(scope.as_bytes());
        signing_input.push(b':');
        signing_input.extend_from_slice(&payload);
        hmac::verify(&key, &signing_input, &signature).map_err(|_| ServiceError::Unauthorized)?;

        serde_json::from_slice(&payload).map_err(|_| ServiceError::Unauthorized)
    }

    fn generate_id(&self, prefix: &str) -> ServiceResult<String> {
        Ok(format!("{prefix}_{}", self.generate_random_token(ID_TOKEN_LEN)?))
    }

    async fn record_failed_login_best_effort(
        &self,
        request_id: &str,
        username: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
        reason: &str,
    ) {
        if let Err(error) =
            self.record_failed_login(request_id, username, user_agent, remote_addr, reason).await
        {
            tracing::warn!(
                request_id = %request_id,
                username = %username,
                error = %error,
                "failed to persist failed-login audit record"
            );
        }
    }

    async fn record_failed_login(
        &self,
        request_id: &str,
        username: &str,
        user_agent: Option<String>,
        remote_addr: Option<String>,
        reason: &str,
    ) -> ServiceResult<()> {
        self.store
            .audit_repository()
            .insert_entry(&NewAuditLogEntry {
                audit_id: self.generate_id("audit")?,
                request_id: request_id.to_string(),
                cluster_id: None,
                actor_id: "anonymous".to_string(),
                action: "auth.login".to_string(),
                resource_type: "user".to_string(),
                resource_id: username.to_string(),
                result: "failed".to_string(),
                details: json!({
                    "username": username,
                    "reason": reason,
                    "user_agent": user_agent,
                    "remote_addr": remote_addr,
                }),
                created_at: Utc::now(),
            })
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))
    }
}

fn unix_time_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().min(u128::from(u64::MAX)) as u64
}
