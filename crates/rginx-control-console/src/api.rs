use std::fmt::{self, Display, Formatter};

use gloo_net::http::{Request, RequestBuilder, Response};
use gloo_storage::{LocalStorage, Storage};
use rginx_control_types::{
    AuthLoginRequest, AuthLoginResponse, ConfigRevisionDetail, ControlPlaneNodeDetailEvent,
    ControlPlaneOverviewEvent, DashboardSummary, NodeDetailResponse, NodeSummary,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{Event, EventSource, EventSourceInit, MessageEvent, RequestCredentials};

#[cfg(target_arch = "wasm32")]
use rginx_control_types::AuthenticatedActor;

const AUTH_TOKEN_STORAGE_KEY: &str = "rginx-control-plane-token";

type MessageListener = (String, Closure<dyn FnMut(MessageEvent)>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiError {
    pub status: Option<u16>,
    pub message: String,
}

impl ApiError {
    pub fn new(status: Option<u16>, message: impl Into<String>) -> Self {
        Self { status, message: message.into() }
    }

    pub fn is_auth_error(&self) -> bool {
        matches!(self.status, Some(401 | 403))
    }
}

impl Display for ApiError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: ErrorDetail,
}

#[derive(Debug, Deserialize)]
struct ErrorDetail {
    message: Option<String>,
}

pub fn stored_auth_token() -> Option<String> {
    LocalStorage::get(AUTH_TOKEN_STORAGE_KEY).ok()
}

pub fn set_stored_auth_token(token: &str) -> Result<(), ApiError> {
    LocalStorage::set(AUTH_TOKEN_STORAGE_KEY, token)
        .map_err(|error| ApiError::new(None, format!("failed to persist auth token: {error}")))
}

pub fn clear_stored_auth_token() {
    LocalStorage::delete(AUTH_TOKEN_STORAGE_KEY);
}

pub async fn login(request: &AuthLoginRequest) -> Result<AuthLoginResponse, ApiError> {
    post_json("/api/v1/auth/login", request, false).await
}

pub async fn logout() -> Result<(), ApiError> {
    post_empty("/api/v1/auth/logout", true).await
}

#[cfg(target_arch = "wasm32")]
pub async fn get_me() -> Result<AuthenticatedActor, ApiError> {
    get_json("/api/v1/auth/me", true).await
}

pub async fn get_dashboard() -> Result<DashboardSummary, ApiError> {
    get_json("/api/v1/dashboard", true).await
}

pub async fn get_nodes() -> Result<Vec<NodeSummary>, ApiError> {
    get_json("/api/v1/nodes", true).await
}

pub async fn get_node_detail(node_id: &str) -> Result<NodeDetailResponse, ApiError> {
    get_json(&format!("/api/v1/nodes/{node_id}"), true).await
}

pub async fn get_revision(revision_id: &str) -> Result<ConfigRevisionDetail, ApiError> {
    get_json(&format!("/api/v1/revisions/{revision_id}"), true).await
}

pub async fn ensure_events_session() -> Result<(), ApiError> {
    post_empty("/api/v1/events/session", true).await
}

pub fn build_events_url(
    node_id: Option<&str>,
    deployment_id: Option<&str>,
    dns_deployment_id: Option<&str>,
) -> String {
    let mut params = Vec::new();
    if let Some(node_id) = node_id {
        params.push(("node_id", node_id.to_string()));
    }
    if let Some(deployment_id) = deployment_id {
        params.push(("deployment_id", deployment_id.to_string()));
    }
    if let Some(dns_deployment_id) = dns_deployment_id {
        params.push(("dns_deployment_id", dns_deployment_id.to_string()));
    }

    let mut url = "/api/v1/events".to_string();
    if !params.is_empty() {
        url.push('?');
        url.push_str(
            &params
                .into_iter()
                .map(|(key, value)| {
                    format!("{}={}", encode_component(key), encode_component(&value))
                })
                .collect::<Vec<_>>()
                .join("&"),
        );
    }
    url
}

pub struct EventStream {
    source: EventSource,
    open_listener: Option<Closure<dyn FnMut(Event)>>,
    error_listener: Option<Closure<dyn FnMut(Event)>>,
    message_listeners: Vec<MessageListener>,
}

impl EventStream {
    pub fn connect(url: &str) -> Result<Self, ApiError> {
        let init = EventSourceInit::new();
        init.set_with_credentials(true);
        let source = EventSource::new_with_event_source_init_dict(url, &init).map_err(|error| {
            ApiError::new(None, format!("failed to open event stream: {:?}", error))
        })?;
        Ok(Self {
            source,
            open_listener: None,
            error_listener: None,
            message_listeners: Vec::new(),
        })
    }

    pub fn on_open(mut self, mut callback: impl FnMut() + 'static) -> Result<Self, ApiError> {
        let closure = Closure::wrap(Box::new(move |_event: Event| {
            callback();
        }) as Box<dyn FnMut(Event)>);
        self.source
            .add_event_listener_with_callback("open", closure.as_ref().unchecked_ref())
            .map_err(|error| {
                ApiError::new(None, format!("failed to register open listener: {:?}", error))
            })?;
        self.open_listener = Some(closure);
        Ok(self)
    }

    pub fn on_error(mut self, mut callback: impl FnMut(u16) + 'static) -> Result<Self, ApiError> {
        let source = self.source.clone();
        let closure = Closure::wrap(Box::new(move |_event: Event| {
            callback(source.ready_state());
        }) as Box<dyn FnMut(Event)>);
        self.source
            .add_event_listener_with_callback("error", closure.as_ref().unchecked_ref())
            .map_err(|error| {
                ApiError::new(None, format!("failed to register error listener: {:?}", error))
            })?;
        self.error_listener = Some(closure);
        Ok(self)
    }

    pub fn on_message(
        mut self,
        event_name: &str,
        mut callback: impl FnMut(String) + 'static,
    ) -> Result<Self, ApiError> {
        let closure = Closure::wrap(Box::new(move |event: MessageEvent| {
            callback(event.data().as_string().unwrap_or_default());
        }) as Box<dyn FnMut(MessageEvent)>);
        self.source
            .add_event_listener_with_callback(event_name, closure.as_ref().unchecked_ref())
            .map_err(|error| {
                ApiError::new(
                    None,
                    format!("failed to register {event_name} listener: {:?}", error),
                )
            })?;
        self.message_listeners.push((event_name.to_string(), closure));
        Ok(self)
    }

    pub fn close(&self) {
        self.source.close();
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        if let Some(listener) = &self.open_listener {
            let _ = self
                .source
                .remove_event_listener_with_callback("open", listener.as_ref().unchecked_ref());
        }
        if let Some(listener) = &self.error_listener {
            let _ = self
                .source
                .remove_event_listener_with_callback("error", listener.as_ref().unchecked_ref());
        }
        for (event_name, listener) in &self.message_listeners {
            let _ = self
                .source
                .remove_event_listener_with_callback(event_name, listener.as_ref().unchecked_ref());
        }
        self.source.close();
    }
}

fn authorized(builder: RequestBuilder) -> RequestBuilder {
    let mut builder = builder.credentials(RequestCredentials::Include);
    if let Some(token) = stored_auth_token() {
        builder = builder.header("Authorization", &format!("Bearer {token}"));
    }
    builder
}

async fn get_json<T>(url: &str, auth: bool) -> Result<T, ApiError>
where
    T: DeserializeOwned,
{
    let builder = Request::get(url);
    let response = if auth {
        authorized(builder).send().await
    } else {
        builder.credentials(RequestCredentials::Include).send().await
    }
    .map_err(|error| ApiError::new(None, format!("request failed: {error}")))?;
    parse_json_response(response).await
}

async fn post_json<Req, Res>(url: &str, body: &Req, auth: bool) -> Result<Res, ApiError>
where
    Req: Serialize,
    Res: DeserializeOwned,
{
    let builder = Request::post(url);
    let builder =
        if auth { authorized(builder) } else { builder.credentials(RequestCredentials::Include) };
    let request = builder
        .json(body)
        .map_err(|error| ApiError::new(None, format!("failed to encode request body: {error}")))?;
    let response = request
        .send()
        .await
        .map_err(|error| ApiError::new(None, format!("request failed: {error}")))?;
    parse_json_response(response).await
}

async fn post_empty(url: &str, auth: bool) -> Result<(), ApiError> {
    let builder = Request::post(url);
    let response = if auth {
        authorized(builder).send().await
    } else {
        builder.credentials(RequestCredentials::Include).send().await
    }
    .map_err(|error| ApiError::new(None, format!("request failed: {error}")))?;
    ensure_success(response).await
}

async fn parse_json_response<T>(response: Response) -> Result<T, ApiError>
where
    T: DeserializeOwned,
{
    if !response.ok() {
        return Err(response_error(response).await);
    }
    response
        .json::<T>()
        .await
        .map_err(|error| ApiError::new(None, format!("failed to decode response: {error}")))
}

async fn ensure_success(response: Response) -> Result<(), ApiError> {
    if response.ok() { Ok(()) } else { Err(response_error(response).await) }
}

async fn response_error(response: Response) -> ApiError {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if let Ok(payload) = serde_json::from_str::<ErrorEnvelope>(&body)
        && let Some(message) = payload.error.message
    {
        return ApiError::new(Some(status), message);
    }
    if body.trim().is_empty() {
        ApiError::new(Some(status), format!("request failed with status {status}"))
    } else {
        ApiError::new(Some(status), body)
    }
}

fn encode_component(value: &str) -> String {
    js_sys::encode_uri_component(value).as_string().unwrap_or_else(|| value.to_string())
}

pub type DashboardStreamEvent = ControlPlaneOverviewEvent;
pub type NodeStreamEvent = ControlPlaneNodeDetailEvent;
