use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fmt,
    fs::File,
    io::Read as _,
    net::IpAddr,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use reqwest::{Client, StatusCode, redirect::Policy};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::{Host, Url};
use zeroize::Zeroizing;

use super::{
    BodyFieldKind, BodyMode, Collection, Environment, RequestDraft, RequestError, ResourceCreator,
    ResponseData, Workspace,
    request::ResponseHeader,
    template::RequestTemplate,
    upstream_management::RequestExecutionMode,
    workspace::{CollectionFolder, EnvironmentVariable, SavedRequest},
};

const LOGIN_RESPONSE_LIMIT_BYTES: usize = 64 * 1024;
const WORKSPACE_RESPONSE_LIMIT_BYTES: usize = 32 * 1024 * 1024;
const LOGIN_TIMEOUT: Duration = Duration::from_secs(20);
const PROXY_TIMEOUT: Duration = Duration::from_secs(75);
const PROXY_POLICY_RESPONSE_LIMIT_BYTES: usize = 64 * 1024;
const PROXY_ENVELOPE_LIMIT_BYTES: usize = 96 * 1024 * 1024;
const MAX_PROXY_BODY_BYTES: usize = 64 * 1024 * 1024;
static NEXT_UPSTREAM_ID: AtomicU64 = AtomicU64::new(0);

/// Persisted connection metadata for every self-hosted Resolved server.
///
/// Authentication material is deliberately absent. Session tokens live in the
/// encrypted credential vault and are addressed by [`UpstreamProfile::id`].
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct UpstreamSettings {
    /// `None` means the built-in local provider is selected.
    pub active_upstream_id: Option<String>,
    pub servers: Vec<UpstreamProfile>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl UpstreamSettings {
    pub fn active(&self) -> Option<&UpstreamProfile> {
        self.active_upstream_id
            .as_deref()
            .and_then(|id| self.server(id))
    }

    pub fn server(&self, id: &str) -> Option<&UpstreamProfile> {
        self.servers.iter().find(|server| server.id == id)
    }

    pub fn server_for_url(&self, base_url: &Url) -> Option<&UpstreamProfile> {
        let normalized = base_url.as_str();
        self.servers
            .iter()
            .find(|server| server.base_url == normalized)
    }

    pub fn select_local(&mut self) {
        self.active_upstream_id = None;
    }

    pub fn select(&mut self, id: &str) -> bool {
        if self.server(id).is_none() {
            return false;
        }
        self.active_upstream_id = Some(id.to_owned());
        true
    }

    pub fn upsert(&mut self, profile: UpstreamProfile) {
        if let Some(existing) = self
            .servers
            .iter_mut()
            .find(|server| server.id == profile.id)
        {
            *existing = profile;
        } else {
            self.servers.push(profile);
        }
        self.servers.sort_by(|left, right| {
            left.display_label()
                .to_lowercase()
                .cmp(&right.display_label().to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
    }

    pub fn remove(&mut self, id: &str) -> Option<UpstreamProfile> {
        let index = self.servers.iter().position(|server| server.id == id)?;
        let removed = self.servers.remove(index);
        if self.active_upstream_id.as_deref() == Some(id) {
            self.active_upstream_id = None;
        }
        Some(removed)
    }

    pub fn has_dangling_selection(&self) -> bool {
        self.active_upstream_id
            .as_deref()
            .is_some_and(|id| self.server(id).is_none())
    }

    pub fn validation_warning(&self) -> Option<String> {
        if self.has_dangling_selection() {
            return Some(
                "The selected upstream server no longer exists; Local is active until another server is selected."
                    .to_owned(),
            );
        }
        let mut ids = BTreeSet::new();
        let mut urls = BTreeSet::new();
        for server in &self.servers {
            if server.id.trim().is_empty() || !ids.insert(server.id.as_str()) {
                return Some(
                    "Upstream settings contain a missing or duplicate server identifier; affected servers cannot be selected safely."
                        .to_owned(),
                );
            }
            let normalized = normalize_upstream_url(&server.base_url).ok();
            if normalized.as_ref().map(Url::as_str) != Some(server.base_url.as_str())
                || !urls.insert(server.base_url.as_str())
            {
                return Some(
                    "Upstream settings contain an invalid or duplicate server URL; affected servers must be forgotten and added again."
                        .to_owned(),
                );
            }
            if server.user_id.trim().is_empty() || server.email.trim().is_empty() {
                return Some(
                    "Upstream settings contain incomplete user metadata; sign in to the affected server again."
                        .to_owned(),
                );
            }
            if server
                .active_workspace_id
                .as_deref()
                .is_some_and(|id| !server.workspaces.iter().any(|workspace| workspace.id == id))
            {
                return Some(
                    "A connected server references a workspace that is no longer available."
                        .to_owned(),
                );
            }
        }
        None
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct UpstreamProfile {
    pub id: String,
    pub base_url: String,
    pub user_id: String,
    pub email: String,
    pub display_name: String,
    pub session_expires_at: DateTime<Utc>,
    pub connected_at: DateTime<Utc>,
    #[serde(default)]
    pub permission_keys: BTreeSet<String>,
    #[serde(default)]
    pub workspaces: Vec<UpstreamWorkspaceSummary>,
    #[serde(default)]
    pub active_workspace_id: Option<String>,
    #[serde(default)]
    pub active_environment_ids: BTreeMap<String, String>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl UpstreamProfile {
    pub fn from_login(
        id: Option<String>,
        base_url: &Url,
        user: &LoginUser,
        expires_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: id.unwrap_or_else(new_upstream_id),
            base_url: base_url.as_str().to_owned(),
            user_id: user.id.clone(),
            email: user.email.clone(),
            display_name: user.display_name.clone(),
            session_expires_at: expires_at,
            connected_at: Utc::now(),
            permission_keys: user.permission_keys(),
            workspaces: Vec::new(),
            active_workspace_id: None,
            active_environment_ids: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }

    pub fn parsed_base_url(&self) -> Option<Url> {
        Url::parse(&self.base_url).ok()
    }

    pub fn display_label(&self) -> String {
        self.parsed_base_url()
            .map(|url| upstream_url_label(&url))
            .unwrap_or_else(|| self.base_url.clone())
    }

    pub fn session_expired(&self, now: DateTime<Utc>) -> bool {
        self.session_expires_at <= now
    }

    pub fn has_permission(&self, permission: &str) -> bool {
        self.permission_keys.contains(permission)
    }

    pub fn replace_permissions(&mut self, permissions: impl IntoIterator<Item = String>) {
        self.permission_keys = permissions.into_iter().collect();
    }

    pub fn active_workspace(&self) -> Option<&UpstreamWorkspaceSummary> {
        self.active_workspace_id
            .as_deref()
            .and_then(|id| self.workspaces.iter().find(|workspace| workspace.id == id))
    }

    pub fn active_environment_id(&self, workspace_id: &str) -> Option<&str> {
        self.active_environment_ids
            .get(workspace_id)
            .map(String::as_str)
    }

    pub fn set_active_environment_id(&mut self, workspace_id: &str, environment_id: Option<&str>) {
        if let Some(environment_id) = environment_id {
            self.active_environment_ids
                .insert(workspace_id.to_owned(), environment_id.to_owned());
        } else {
            self.active_environment_ids.remove(workspace_id);
        }
    }

    pub fn replace_workspaces(
        &mut self,
        workspaces: impl IntoIterator<Item = UpstreamWorkspaceSummary>,
        active_workspace_id: Option<String>,
    ) {
        self.workspaces = workspaces.into_iter().collect();
        self.workspaces.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        self.active_workspace_id = active_workspace_id
            .filter(|id| self.workspaces.iter().any(|workspace| workspace.id == *id));
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct UpstreamWorkspaceSummary {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct UpstreamUserSummary {
    pub id: String,
    pub email: String,
    pub display_name: String,
}

impl From<UpstreamUserSummary> for ResourceCreator {
    fn from(user: UpstreamUserSummary) -> Self {
        Self {
            id: user.id,
            email: user.email,
            display_name: user.display_name,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct UpstreamWorkspaceView {
    pub id: String,
    pub name: String,
    pub user_ids: Vec<String>,
    pub collections: Vec<UpstreamCollectionView>,
    pub created_by: Option<UpstreamUserSummary>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct UpstreamEnvironmentView {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    pub variables: Vec<UpstreamEnvironmentVariableView>,
    pub created_by: Option<UpstreamUserSummary>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl UpstreamEnvironmentView {
    pub fn into_local(self) -> Environment {
        Environment {
            id: self.id,
            name: self.name,
            created_by: self.created_by.map(Into::into),
            variables: self
                .variables
                .into_iter()
                .map(UpstreamEnvironmentVariableView::into_local)
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct UpstreamEnvironmentVariableView {
    pub id: String,
    pub environment_id: String,
    pub key: String,
    pub value: String,
    pub enabled: bool,
    pub secret: bool,
    pub created_by: Option<UpstreamUserSummary>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl UpstreamEnvironmentVariableView {
    pub fn into_local(self) -> EnvironmentVariable {
        EnvironmentVariable {
            id: self.id,
            key: self.key,
            value: self.value,
            enabled: self.enabled,
            secret: self.secret,
            created_by: self.created_by.map(Into::into),
        }
    }
}

impl UpstreamWorkspaceView {
    pub fn summary(&self) -> UpstreamWorkspaceSummary {
        UpstreamWorkspaceSummary {
            id: self.id.clone(),
            name: self.name.clone(),
        }
    }

    pub fn into_local_workspace(self) -> Workspace {
        let collections = self
            .collections
            .into_iter()
            .map(|root| {
                let mut folders = Vec::new();
                let mut requests = root
                    .requests
                    .into_iter()
                    .map(|request| request.into_local(None))
                    .collect();
                flatten_remote_collections(root.sub_collections, None, &mut folders, &mut requests);
                Collection {
                    id: root.id,
                    name: root.name,
                    created_by: root.created_by.map(Into::into),
                    folders,
                    requests,
                }
            })
            .collect();
        Workspace {
            created_by: self.created_by.map(Into::into),
            collections,
            environments: Vec::new(),
            active_environment_id: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct UpstreamCollectionView {
    pub id: String,
    pub workspace_id: String,
    pub parent_collection_id: Option<String>,
    pub name: String,
    pub user_ids: Vec<String>,
    pub sub_collections: Vec<UpstreamCollectionView>,
    #[serde(default)]
    pub requests: Vec<UpstreamSavedRequestView>,
    pub created_by: Option<UpstreamUserSummary>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct UpstreamSavedRequestView {
    pub id: String,
    pub collection_id: String,
    pub name: String,
    pub definition: RequestTemplate,
    pub created_by: Option<UpstreamUserSummary>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl UpstreamSavedRequestView {
    pub fn into_local(self, folder_id: Option<String>) -> SavedRequest {
        SavedRequest {
            id: self.id,
            name: self.name,
            created_by: self.created_by.map(Into::into),
            folder_id,
            definition: self.definition,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct LoginUser {
    pub id: String,
    pub email: String,
    pub display_name: String,
    pub active: bool,
    #[serde(default)]
    pub roles: Vec<LoginRole>,
}

impl LoginUser {
    pub fn permission_keys(&self) -> BTreeSet<String> {
        self.roles
            .iter()
            .flat_map(|role| role.permissions.iter())
            .map(|permission| permission.key.clone())
            .collect()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct LoginRole {
    #[serde(default)]
    pub permissions: Vec<LoginPermission>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct LoginPermission {
    pub key: String,
}

pub struct UpstreamLoginResult {
    pub base_url: Url,
    pub token: Zeroizing<String>,
    pub expires_at: DateTime<Utc>,
    pub user: LoginUser,
}

impl fmt::Debug for UpstreamLoginResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UpstreamLoginResult")
            .field("base_url", &self.base_url)
            .field("token", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .field("user", &self.user)
            .finish()
    }
}

#[derive(Debug, Error)]
pub enum UpstreamUrlError {
    #[error("enter a server URL")]
    Empty,
    #[error("the server URL is invalid: {0}")]
    Invalid(#[from] url::ParseError),
    #[error("the server URL must use HTTPS (HTTP is allowed only for loopback servers)")]
    InsecureTransport,
    #[error("the server URL must use HTTP or HTTPS")]
    UnsupportedScheme,
    #[error("the server URL must include a host")]
    MissingHost,
    #[error("the server URL must not contain a username or password")]
    EmbeddedCredentials,
    #[error("the server URL must not contain a query or fragment")]
    QueryOrFragment,
}

#[derive(Debug, Error)]
pub enum UpstreamLoginError {
    #[error(transparent)]
    InvalidUrl(#[from] UpstreamUrlError),
    #[error("could not create the secure login client: {0}")]
    Client(reqwest::Error),
    #[error("{0}")]
    CryptoProvider(&'static str),
    #[error("could not reach the server: {0}")]
    Transport(reqwest::Error),
    #[error("the server redirected the login request; enter its canonical URL")]
    Redirected,
    #[error("the server returned more than {limit_bytes} bytes during login")]
    ResponseTooLarge { limit_bytes: usize },
    #[error("the server returned an invalid login response: {0}")]
    InvalidResponse(String),
    #[error("{message}")]
    Rejected { status: StatusCode, message: String },
}

#[derive(Debug, Error)]
pub enum UpstreamWorkspaceError {
    #[error("could not reach the server: {0}")]
    Transport(reqwest::Error),
    #[error("the server redirected the workspace request")]
    Redirected,
    #[error("the server returned more than {limit_bytes} bytes")]
    ResponseTooLarge { limit_bytes: usize },
    #[error("the server returned an invalid workspace response: {0}")]
    InvalidResponse(String),
    #[error("{message}")]
    Rejected { status: StatusCode, message: String },
}

#[derive(Serialize)]
struct LoginRequest<'a> {
    email: &'a str,
    password: &'a str,
}

#[derive(Serialize)]
struct CreateWorkspaceRequest<'a> {
    name: &'a str,
}

#[derive(Serialize)]
struct MoveCollectionRequest<'a> {
    parent_collection_id: Option<&'a str>,
}

#[derive(Serialize)]
struct MoveSavedRequestRequest<'a> {
    target_collection_id: &'a str,
}

#[derive(Serialize)]
struct CreateCollectionRequest<'a> {
    name: &'a str,
    parent_collection_id: Option<&'a str>,
}

#[derive(Serialize)]
struct SaveRequestRequest<'a> {
    name: &'a str,
    definition: &'a RequestTemplate,
}

#[derive(Serialize)]
struct EnvironmentRequest<'a> {
    name: &'a str,
}

#[derive(Serialize)]
struct CreateEnvironmentVariableRequest<'a> {
    key: &'a str,
    value: &'a str,
    enabled: bool,
    secret: bool,
}

#[derive(Serialize)]
struct UpdateEnvironmentVariableRequest<'a> {
    key: &'a str,
    enabled: bool,
    secret: bool,
}

#[derive(Serialize)]
struct PutEnvironmentVariableValueRequest<'a> {
    value: &'a str,
}

#[derive(Deserialize)]
struct LoginEnvelope {
    success: bool,
    data: Option<LoginData>,
    error: Option<LoginErrorBody>,
}

#[derive(Deserialize)]
struct LoginData {
    token: String,
    token_type: String,
    expires_at: DateTime<Utc>,
    user: LoginUser,
}

#[derive(Deserialize)]
struct LoginErrorBody {
    #[serde(default)]
    code: String,
    message: String,
    #[serde(default)]
    fields: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct WorkspaceEnvelope<T> {
    success: bool,
    data: Option<T>,
    error: Option<LoginErrorBody>,
}

fn format_upstream_error(error: LoginErrorBody) -> String {
    let mut fields = error
        .fields
        .into_iter()
        .filter(|(_, message)| !message.trim().is_empty())
        .map(|(field, message)| format!("{field}: {message}"))
        .collect::<Vec<_>>();
    fields.sort();
    if fields.is_empty() {
        error.message
    } else {
        format!("{} ({})", error.message, fields.join(", "))
    }
}

pub fn build_upstream_client() -> Result<Client, UpstreamLoginError> {
    crate::tls::install_crypto_provider().map_err(UpstreamLoginError::CryptoProvider)?;
    Client::builder()
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("RESOLVED_BUILD_VERSION")
        ))
        // Never replay a login/password body to a redirect target.
        .redirect(Policy::none())
        .timeout(LOGIN_TIMEOUT)
        .build()
        .map_err(UpstreamLoginError::Client)
}

pub fn build_upstream_execution_client() -> Result<Client, RequestError> {
    crate::tls::install_crypto_provider()
        .map_err(|error| RequestError::TaskFailed(error.to_owned()))?;
    Client::builder()
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("RESOLVED_BUILD_VERSION")
        ))
        .redirect(Policy::none())
        .timeout(PROXY_TIMEOUT)
        .build()
        .map_err(RequestError::Transport)
}

#[derive(Deserialize)]
struct ProxyExecutionPolicy {
    mode: RequestExecutionMode,
    #[serde(default)]
    cookie_jar: bool,
}

pub async fn get_upstream_execution_policy(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
) -> Result<RequestExecutionMode, RequestError> {
    Ok(load_execution_policy(client, base_url, bearer_token)
        .await?
        .mode)
}
async fn load_execution_policy(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
) -> Result<ProxyExecutionPolicy, RequestError> {
    let endpoint = base_url.join("api/v1/request-execution").map_err(|error| {
        RequestError::Upstream(format!(
            "the server execution policy URL is invalid: {error}"
        ))
    })?;
    let mut response = client
        .get(endpoint)
        .bearer_auth(bearer_token)
        .send()
        .await
        .map_err(RequestError::Transport)?;
    if response.status().is_redirection() {
        return Err(RequestError::Upstream(
            "the server redirected the request execution policy".to_owned(),
        ));
    }
    let status = response.status();
    // Servers released before request proxying do not expose a policy route;
    // their server workspaces retain the original local-execution behavior.
    if status == StatusCode::NOT_FOUND {
        return Ok(ProxyExecutionPolicy {
            mode: RequestExecutionMode::Local,
            cookie_jar: false,
        });
    }
    if response
        .content_length()
        .is_some_and(|length| length > PROXY_POLICY_RESPONSE_LIMIT_BYTES as u64)
    {
        return Err(RequestError::Upstream(
            "the server returned an oversized request execution policy".to_owned(),
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(RequestError::Transport)? {
        if body.len().saturating_add(chunk.len()) > PROXY_POLICY_RESPONSE_LIMIT_BYTES {
            return Err(RequestError::Upstream(
                "the server returned an oversized request execution policy".to_owned(),
            ));
        }
        body.extend_from_slice(&chunk);
    }

    let envelope: WorkspaceEnvelope<ProxyExecutionPolicy> =
        serde_json::from_slice(&body).map_err(|error| {
            RequestError::Upstream(format!(
                "the server returned an invalid request execution policy: {error}"
            ))
        })?;
    if !status.is_success() || !envelope.success {
        let message = envelope
            .error
            .map(format_upstream_error)
            .filter(|message| !message.trim().is_empty())
            .unwrap_or_else(|| format!("could not load server execution policy: HTTP {status}"));
        return Err(RequestError::Upstream(message));
    }
    envelope.data.ok_or_else(|| {
        RequestError::Upstream(
            "the server response did not include its request execution policy".to_owned(),
        )
    })
}

#[derive(Serialize)]
struct ProxyAllowlistEntry<'a> {
    kind: &'a str,
    value: &'a str,
}

pub async fn add_upstream_proxy_allowlist_entry(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    kind: &str,
    value: &str,
) -> Result<(), RequestError> {
    let endpoint = base_url
        .join("api/v1/request-execution/allowlist")
        .map_err(|error| {
            RequestError::Upstream(format!("the proxy allowlist URL is invalid: {error}"))
        })?;
    let mut response = client
        .post(endpoint)
        .bearer_auth(bearer_token)
        .json(&ProxyAllowlistEntry { kind, value })
        .send()
        .await
        .map_err(RequestError::Transport)?;
    if response.status().is_redirection() {
        return Err(RequestError::Upstream(
            "the server redirected the proxy allowlist endpoint".to_owned(),
        ));
    }
    let status = response.status();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(RequestError::Transport)? {
        if body.len().saturating_add(chunk.len()) > PROXY_POLICY_RESPONSE_LIMIT_BYTES {
            return Err(RequestError::Upstream(
                "the server returned an oversized proxy allowlist response".to_owned(),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    let envelope: WorkspaceEnvelope<serde_json::Value> =
        serde_json::from_slice(&body).map_err(|error| {
            RequestError::Upstream(format!(
                "the server returned an invalid proxy allowlist response: {error}"
            ))
        })?;
    if !status.is_success() || !envelope.success {
        let message = envelope
            .error
            .map(format_upstream_error)
            .filter(|message| !message.trim().is_empty())
            .unwrap_or_else(|| format!("could not update the proxy allowlist: HTTP {status}"));
        return Err(RequestError::Upstream(message));
    }
    Ok(())
}

#[cfg(test)]
pub async fn send_request_for_upstream_workspace(
    upstream_client: &Client,
    local_client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    request: RequestDraft,
) -> Result<ResponseData, RequestError> {
    match get_upstream_execution_policy(upstream_client, base_url, bearer_token).await? {
        RequestExecutionMode::Local => super::request::send_request(local_client, request).await,
        RequestExecutionMode::Server => {
            execute_upstream_request(
                upstream_client,
                base_url,
                bearer_token,
                workspace_id,
                request,
            )
            .await
        }
    }
}

pub async fn send_request_for_upstream_workspace_with_cookies(
    upstream_client: &Client,
    local_client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    request: RequestDraft,
    jar: &super::CookieJar,
) -> Result<ResponseData, RequestError> {
    jar.synchronized().await.map_err(RequestError::Upstream)?;
    let policy = load_execution_policy(upstream_client, base_url, bearer_token).await?;
    match policy.mode {
        RequestExecutionMode::Local => {
            let result = super::request::send_request(local_client, request).await;
            jar.synchronized().await.map_err(|e| {
                RequestError::Upstream(format!(
                    "Request completed, but cookie synchronization failed: {e}"
                ))
            })?;
            result
        }
        RequestExecutionMode::Server => {
            if jar.enabled() && !policy.cookie_jar {
                return Err(RequestError::Upstream(
                    "Update this server to support encrypted cookie jars.".into(),
                ));
            }
            let result = execute_upstream_request_with_cookies(
                upstream_client,
                base_url,
                bearer_token,
                workspace_id,
                request,
                jar.enabled(),
            )
            .await;
            jar.refresh().await.map_err(|e| {
                RequestError::Upstream(format!(
                    "Request may have been sent, but cookie refresh failed: {e}"
                ))
            })?;
            result
        }
    }
}

#[derive(Serialize)]
struct ProxyExecuteRequest {
    use_cookie_jar: bool,
    method: String,
    url: String,
    headers: Vec<ProxyHeader>,
    body: ProxyBody,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ProxyHeader {
    name: String,
    value: String,
}

#[derive(Serialize)]
struct ProxyBody {
    mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data_base64: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    fields: Vec<ProxyBodyField>,
}

#[derive(Serialize)]
struct ProxyBodyField {
    name: String,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_base64: Option<String>,
}

#[derive(Deserialize)]
struct ProxyExecuteResult {
    status: u16,
    status_text: String,
    http_version: String,
    final_url: String,
    headers: Vec<ProxyHeader>,
    #[serde(default)]
    content_type: String,
    body_base64: String,
    duration_micros: u64,
}

#[cfg(test)]
pub async fn execute_upstream_request(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    request: RequestDraft,
) -> Result<ResponseData, RequestError> {
    execute_upstream_request_with_cookies(
        client,
        base_url,
        bearer_token,
        workspace_id,
        request,
        false,
    )
    .await
}
async fn execute_upstream_request_with_cookies(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    request: RequestDraft,
    use_cookie_jar: bool,
) -> Result<ResponseData, RequestError> {
    let mut payload = proxy_request_payload(request).await?;
    payload.use_cookie_jar = use_cookie_jar;

    let endpoint = base_url
        .join(&format!("api/v1/workspaces/{workspace_id}/execute"))
        .map_err(|error| {
            RequestError::Upstream(format!("the server execution URL is invalid: {error}"))
        })?;
    let response = client
        .post(endpoint)
        .bearer_auth(bearer_token)
        .json(&payload)
        .send()
        .await
        .map_err(RequestError::Transport)?;
    parse_proxy_response(response).await
}

async fn proxy_request_payload(request: RequestDraft) -> Result<ProxyExecuteRequest, RequestError> {
    let mut headers = request
        .headers
        .iter()
        .filter(|header| header.enabled && !header.name.trim().is_empty())
        .map(|header| ProxyHeader {
            name: header.name.clone(),
            value: header.value.clone(),
        })
        .collect::<Vec<_>>();
    if !headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case("user-agent"))
    {
        headers.push(ProxyHeader {
            name: "User-Agent".to_owned(),
            value: concat!("resolved/", env!("RESOLVED_BUILD_VERSION")).to_owned(),
        });
    }

    let body = match request.body_mode {
        BodyMode::None => ProxyBody {
            mode: "none",
            raw_content_type: None,
            data_base64: None,
            fields: Vec::new(),
        },
        BodyMode::Raw => {
            ensure_proxy_body_limit(request.body.len())?;
            ProxyBody {
                mode: "raw",
                raw_content_type: (!request.body.is_empty())
                    .then(|| request.raw_body_language.content_type().to_owned()),
                data_base64: Some(BASE64_STANDARD.encode(request.body.as_bytes())),
                fields: Vec::new(),
            }
        }
        BodyMode::FormUrlEncoded => ProxyBody {
            mode: "form_url_encoded",
            raw_content_type: None,
            data_base64: None,
            fields: request
                .body_fields
                .iter()
                .filter(|field| field.enabled && !field.name.trim().is_empty())
                .map(|field| ProxyBodyField {
                    name: field.name.clone(),
                    kind: "text",
                    value: Some(field.value.clone()),
                    filename: None,
                    content_base64: None,
                })
                .collect(),
        },
        BodyMode::MultipartFormData => {
            let mut fields = Vec::new();
            let mut materialized_bytes = 0usize;
            for field in request
                .body_fields
                .iter()
                .filter(|field| field.enabled && !field.name.trim().is_empty())
            {
                match field.kind {
                    BodyFieldKind::Text => {
                        materialized_bytes = materialized_bytes.saturating_add(field.value.len());
                        ensure_proxy_body_limit(materialized_bytes)?;
                        fields.push(ProxyBodyField {
                            name: field.name.clone(),
                            kind: "text",
                            value: Some(field.value.clone()),
                            filename: None,
                            content_base64: None,
                        });
                    }
                    BodyFieldKind::File => {
                        let Some(path) = field.file_path() else {
                            continue;
                        };
                        let path = path.to_path_buf();
                        let filename = path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "file".to_owned());
                        let field_name = field.name.clone();
                        let remaining = MAX_PROXY_BODY_BYTES.saturating_sub(materialized_bytes);
                        let task_path = path.clone();
                        let content = tokio::task::spawn_blocking(move || {
                            read_proxy_file(&task_path, remaining)
                        })
                        .await
                        .map_err(|error| RequestError::TaskFailed(error.to_string()))?
                        .map_err(|reason| {
                            RequestError::MultipartFileRead {
                                field_name: field_name.clone(),
                                path: path.display().to_string(),
                                reason,
                            }
                        })?;
                        materialized_bytes = materialized_bytes.saturating_add(content.len());
                        ensure_proxy_body_limit(materialized_bytes)?;
                        fields.push(ProxyBodyField {
                            name: field_name,
                            kind: "file",
                            value: None,
                            filename: Some(filename),
                            content_base64: Some(BASE64_STANDARD.encode(content)),
                        });
                    }
                }
            }
            ProxyBody {
                mode: "multipart_form_data",
                raw_content_type: None,
                data_base64: None,
                fields,
            }
        }
    };

    Ok(ProxyExecuteRequest {
        use_cookie_jar: false,
        method: request.method,
        url: request.url,
        headers,
        body,
    })
}

fn read_proxy_file(path: &PathBuf, limit: usize) -> std::io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let take_limit = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    let mut content = Vec::new();
    file.take(take_limit).read_to_end(&mut content)?;
    if content.len() > limit {
        return Err(std::io::Error::other(format!(
            "file exceeds the {MAX_PROXY_BODY_BYTES}-byte proxied request limit"
        )));
    }
    Ok(content)
}

fn ensure_proxy_body_limit(size: usize) -> Result<(), RequestError> {
    if size > MAX_PROXY_BODY_BYTES {
        return Err(RequestError::Upstream(format!(
            "request body exceeds the {MAX_PROXY_BODY_BYTES}-byte proxied request limit"
        )));
    }
    Ok(())
}

async fn parse_proxy_response(
    mut response: reqwest::Response,
) -> Result<ResponseData, RequestError> {
    if response.status().is_redirection() {
        return Err(RequestError::Upstream(
            "the server redirected the proxied request endpoint".to_owned(),
        ));
    }
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > PROXY_ENVELOPE_LIMIT_BYTES as u64)
    {
        return Err(RequestError::Upstream(
            "the server returned an oversized proxied response".to_owned(),
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(RequestError::Transport)? {
        if body.len().saturating_add(chunk.len()) > PROXY_ENVELOPE_LIMIT_BYTES {
            return Err(RequestError::Upstream(
                "the server returned an oversized proxied response".to_owned(),
            ));
        }
        body.extend_from_slice(&chunk);
    }

    let envelope: WorkspaceEnvelope<ProxyExecuteResult> =
        serde_json::from_slice(&body).map_err(|error| {
            RequestError::Upstream(format!(
                "the server returned an invalid proxied response: {error}"
            ))
        })?;
    if !status.is_success() || !envelope.success {
        if let Some(error) = envelope.error.as_ref()
            && error.code == "proxy_destination_blocked"
        {
            return Err(RequestError::ProxyDestinationBlocked {
                request: error.fields.get("request").cloned().unwrap_or_default(),
                address: error.fields.get("address").cloned().unwrap_or_default(),
                reason: error.fields.get("reason").cloned().unwrap_or_default(),
            });
        }
        let message = envelope
            .error
            .map(format_upstream_error)
            .filter(|message| !message.trim().is_empty())
            .unwrap_or_else(|| format!("server execution failed with HTTP {status}"));
        return Err(RequestError::Upstream(message));
    }
    let data = envelope.data.ok_or_else(|| {
        RequestError::Upstream("the server response did not include execution data".to_owned())
    })?;
    let decoded = BASE64_STANDARD.decode(data.body_base64).map_err(|error| {
        RequestError::Upstream(format!(
            "the server returned invalid response data: {error}"
        ))
    })?;
    if decoded.len() > MAX_PROXY_BODY_BYTES {
        return Err(RequestError::ResponseBodyTooLarge {
            limit_bytes: MAX_PROXY_BODY_BYTES,
        });
    }

    Ok(ResponseData {
        status: data.status,
        status_text: data.status_text,
        http_version: data.http_version,
        final_url: data.final_url,
        headers: data
            .headers
            .into_iter()
            .map(|header| ResponseHeader {
                name: header.name,
                value: header.value,
            })
            .collect(),
        content_type: (!data.content_type.is_empty()).then_some(data.content_type),
        body: Bytes::from(decoded),
        duration: Duration::from_micros(data.duration_micros),
    })
}

pub async fn login_upstream(
    client: &Client,
    base_url: Url,
    login: String,
    password: Zeroizing<String>,
) -> Result<UpstreamLoginResult, UpstreamLoginError> {
    let endpoint = base_url
        .join("api/v1/auth/login")
        .map_err(UpstreamUrlError::Invalid)?;
    let mut response = client
        .post(endpoint)
        .json(&LoginRequest {
            email: login.trim(),
            password: password.as_str(),
        })
        .send()
        .await
        .map_err(UpstreamLoginError::Transport)?;

    if response.status().is_redirection() {
        return Err(UpstreamLoginError::Redirected);
    }

    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > LOGIN_RESPONSE_LIMIT_BYTES as u64)
    {
        return Err(UpstreamLoginError::ResponseTooLarge {
            limit_bytes: LOGIN_RESPONSE_LIMIT_BYTES,
        });
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(UpstreamLoginError::Transport)?
    {
        let next_len = body.len().saturating_add(chunk.len());
        if next_len > LOGIN_RESPONSE_LIMIT_BYTES {
            return Err(UpstreamLoginError::ResponseTooLarge {
                limit_bytes: LOGIN_RESPONSE_LIMIT_BYTES,
            });
        }
        body.extend_from_slice(&chunk);
    }

    parse_login_response(status, &body, base_url)
}

pub async fn list_upstream_workspaces(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
) -> Result<Vec<UpstreamWorkspaceView>, UpstreamWorkspaceError> {
    let endpoint = base_url
        .join("api/v1/workspaces")
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    let response = client
        .get(endpoint)
        .bearer_auth(bearer_token)
        .send()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?;
    parse_workspace_response(response).await
}

pub async fn get_upstream_user(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
) -> Result<LoginUser, UpstreamWorkspaceError> {
    let endpoint = base_url
        .join("api/v1/auth/me")
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    let response = client
        .get(endpoint)
        .bearer_auth(bearer_token)
        .send()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?;
    parse_workspace_response(response).await
}

pub async fn get_upstream_workspace(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
) -> Result<UpstreamWorkspaceView, UpstreamWorkspaceError> {
    let endpoint = base_url
        .join(&format!("api/v1/workspaces/{workspace_id}"))
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    let response = client
        .get(endpoint)
        .bearer_auth(bearer_token)
        .send()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?;
    parse_workspace_response(response).await
}

pub async fn create_upstream_workspace(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    name: &str,
) -> Result<UpstreamWorkspaceView, UpstreamWorkspaceError> {
    let endpoint = base_url
        .join("api/v1/workspaces")
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    let response = client
        .post(endpoint)
        .bearer_auth(bearer_token)
        .json(&CreateWorkspaceRequest { name })
        .send()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?;
    parse_workspace_response(response).await
}

pub async fn update_upstream_workspace(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    name: &str,
) -> Result<UpstreamWorkspaceView, UpstreamWorkspaceError> {
    let endpoint = base_url
        .join(&format!("api/v1/workspaces/{workspace_id}"))
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    let response = client
        .patch(endpoint)
        .bearer_auth(bearer_token)
        .json(&CreateWorkspaceRequest { name })
        .send()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?;
    parse_workspace_response(response).await
}

pub async fn delete_upstream_workspace(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
) -> Result<(), UpstreamWorkspaceError> {
    let endpoint = base_url
        .join(&format!("api/v1/workspaces/{workspace_id}"))
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    let response = client
        .delete(endpoint)
        .bearer_auth(bearer_token)
        .send()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?;
    parse_workspace_response::<serde_json::Value>(response)
        .await
        .map(|_| ())
}

pub async fn create_upstream_collection(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    name: &str,
    parent_collection_id: Option<&str>,
) -> Result<UpstreamCollectionView, UpstreamWorkspaceError> {
    let endpoint = base_url
        .join(&format!("api/v1/workspaces/{workspace_id}/collections"))
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    let response = client
        .post(endpoint)
        .bearer_auth(bearer_token)
        .json(&CreateCollectionRequest {
            name,
            parent_collection_id,
        })
        .send()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?;
    parse_workspace_response(response).await
}

pub async fn update_upstream_collection(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    collection_id: &str,
    name: &str,
) -> Result<UpstreamCollectionView, UpstreamWorkspaceError> {
    let endpoint = base_url
        .join(&format!(
            "api/v1/workspaces/{workspace_id}/collections/{collection_id}"
        ))
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    let response = client
        .patch(endpoint)
        .bearer_auth(bearer_token)
        .json(&CreateWorkspaceRequest { name })
        .send()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?;
    parse_workspace_response(response).await
}

pub async fn move_upstream_collection(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    collection_id: &str,
    parent_collection_id: Option<&str>,
) -> Result<UpstreamCollectionView, UpstreamWorkspaceError> {
    let endpoint = base_url
        .join(&format!(
            "api/v1/workspaces/{workspace_id}/collections/{collection_id}/parent"
        ))
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    let response = client
        .put(endpoint)
        .bearer_auth(bearer_token)
        .json(&MoveCollectionRequest {
            parent_collection_id,
        })
        .send()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?;
    parse_workspace_response(response).await
}

pub async fn delete_upstream_collection(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    collection_id: &str,
) -> Result<(), UpstreamWorkspaceError> {
    let endpoint = base_url
        .join(&format!(
            "api/v1/workspaces/{workspace_id}/collections/{collection_id}"
        ))
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    let response = client
        .delete(endpoint)
        .bearer_auth(bearer_token)
        .send()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?;
    parse_workspace_response::<serde_json::Value>(response)
        .await
        .map(|_| ())
}

pub async fn list_upstream_environments(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
) -> Result<Vec<UpstreamEnvironmentView>, UpstreamWorkspaceError> {
    let endpoint = base_url
        .join(&format!("api/v1/workspaces/{workspace_id}/environments"))
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    let response = client
        .get(endpoint)
        .bearer_auth(bearer_token)
        .send()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?;
    parse_workspace_response(response).await
}

pub async fn create_upstream_environment(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    name: &str,
) -> Result<UpstreamEnvironmentView, UpstreamWorkspaceError> {
    let endpoint = base_url
        .join(&format!("api/v1/workspaces/{workspace_id}/environments"))
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    let response = client
        .post(endpoint)
        .bearer_auth(bearer_token)
        .json(&EnvironmentRequest { name })
        .send()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?;
    parse_workspace_response(response).await
}

pub async fn update_upstream_environment(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    environment_id: &str,
    name: &str,
) -> Result<UpstreamEnvironmentView, UpstreamWorkspaceError> {
    let endpoint = base_url
        .join(&format!(
            "api/v1/workspaces/{workspace_id}/environments/{environment_id}"
        ))
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    let response = client
        .patch(endpoint)
        .bearer_auth(bearer_token)
        .json(&EnvironmentRequest { name })
        .send()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?;
    parse_workspace_response(response).await
}

pub async fn delete_upstream_environment(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    environment_id: &str,
) -> Result<(), UpstreamWorkspaceError> {
    let endpoint = base_url
        .join(&format!(
            "api/v1/workspaces/{workspace_id}/environments/{environment_id}"
        ))
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    let response = client
        .delete(endpoint)
        .bearer_auth(bearer_token)
        .send()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?;
    parse_workspace_response::<serde_json::Value>(response)
        .await
        .map(|_| ())
}

#[allow(clippy::too_many_arguments)]
pub async fn create_upstream_environment_variable(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    environment_id: &str,
    variable: &EnvironmentVariable,
) -> Result<UpstreamEnvironmentVariableView, UpstreamWorkspaceError> {
    let endpoint = base_url
        .join(&format!(
            "api/v1/workspaces/{workspace_id}/environments/{environment_id}/variables"
        ))
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    let response = client
        .post(endpoint)
        .bearer_auth(bearer_token)
        .json(&CreateEnvironmentVariableRequest {
            key: &variable.key,
            value: &variable.value,
            enabled: variable.enabled,
            secret: variable.secret,
        })
        .send()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?;
    parse_workspace_response(response).await
}

#[allow(clippy::too_many_arguments)]
pub async fn update_upstream_environment_variable(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    environment_id: &str,
    variable_id: &str,
    variable: &EnvironmentVariable,
) -> Result<UpstreamEnvironmentVariableView, UpstreamWorkspaceError> {
    let endpoint = base_url
        .join(&format!(
            "api/v1/workspaces/{workspace_id}/environments/{environment_id}/variables/{variable_id}"
        ))
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    let response = client
        .patch(endpoint)
        .bearer_auth(bearer_token)
        .json(&UpdateEnvironmentVariableRequest {
            key: &variable.key,
            enabled: variable.enabled,
            secret: variable.secret,
        })
        .send()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?;
    parse_workspace_response(response).await
}

pub async fn put_upstream_environment_variable_value(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    environment_id: &str,
    variable_id: &str,
    value: &str,
) -> Result<UpstreamEnvironmentVariableView, UpstreamWorkspaceError> {
    let endpoint = base_url
        .join(&format!(
            "api/v1/workspaces/{workspace_id}/environments/{environment_id}/variables/{variable_id}/value"
        ))
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    let response = client
        .put(endpoint)
        .bearer_auth(bearer_token)
        .json(&PutEnvironmentVariableValueRequest { value })
        .send()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?;
    parse_workspace_response(response).await
}

pub async fn delete_upstream_environment_variable(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    environment_id: &str,
    variable_id: &str,
) -> Result<(), UpstreamWorkspaceError> {
    let endpoint = base_url
        .join(&format!(
            "api/v1/workspaces/{workspace_id}/environments/{environment_id}/variables/{variable_id}"
        ))
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    let response = client
        .delete(endpoint)
        .bearer_auth(bearer_token)
        .send()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?;
    parse_workspace_response::<serde_json::Value>(response)
        .await
        .map(|_| ())
}

#[allow(clippy::too_many_arguments)]
pub async fn save_upstream_environment(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    baseline: &Environment,
    draft: &Environment,
) -> Result<(), UpstreamWorkspaceError> {
    if baseline.id != draft.id {
        return Err(UpstreamWorkspaceError::InvalidResponse(
            "the edited environment no longer matches the server environment".to_owned(),
        ));
    }

    if baseline.name != draft.name {
        update_upstream_environment(
            client,
            base_url,
            bearer_token,
            workspace_id,
            &baseline.id,
            &draft.name,
        )
        .await?;
    }

    // Index both variable lists by id once so classification is a single pass
    // over each side instead of an O(V^2) scan per save. `or_insert` keeps the
    // original first-match lookup semantics if a duplicated id ever appears.
    let baseline_variables: HashMap<&str, &EnvironmentVariable> =
        baseline
            .variables
            .iter()
            .fold(HashMap::new(), |mut by_id, variable| {
                by_id.entry(variable.id.as_str()).or_insert(variable);
                by_id
            });
    let draft_variable_ids: HashSet<&str> = draft
        .variables
        .iter()
        .map(|variable| variable.id.as_str())
        .collect();

    for variable in &baseline.variables {
        if !draft_variable_ids.contains(variable.id.as_str()) {
            delete_upstream_environment_variable(
                client,
                base_url,
                bearer_token,
                workspace_id,
                &baseline.id,
                &variable.id,
            )
            .await?;
        }
    }

    for variable in &draft.variables {
        let Some(previous) = baseline_variables.get(variable.id.as_str()) else {
            create_upstream_environment_variable(
                client,
                base_url,
                bearer_token,
                workspace_id,
                &baseline.id,
                variable,
            )
            .await?;
            continue;
        };
        if previous.key != variable.key
            || previous.enabled != variable.enabled
            || previous.secret != variable.secret
        {
            update_upstream_environment_variable(
                client,
                base_url,
                bearer_token,
                workspace_id,
                &baseline.id,
                &variable.id,
                variable,
            )
            .await?;
        }
        if previous.value != variable.value {
            put_upstream_environment_variable_value(
                client,
                base_url,
                bearer_token,
                workspace_id,
                &baseline.id,
                &variable.id,
                &variable.value,
            )
            .await?;
        }
    }

    Ok(())
}

pub async fn create_upstream_saved_request(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    collection_id: &str,
    name: &str,
    definition: &RequestTemplate,
) -> Result<UpstreamSavedRequestView, UpstreamWorkspaceError> {
    let endpoint = base_url
        .join(&format!(
            "api/v1/workspaces/{workspace_id}/collections/{collection_id}/requests"
        ))
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    let response = client
        .post(endpoint)
        .bearer_auth(bearer_token)
        .json(&SaveRequestRequest { name, definition })
        .send()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?;
    parse_workspace_response(response).await
}

#[allow(clippy::too_many_arguments)]
pub async fn update_upstream_saved_request(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    collection_id: &str,
    request_id: &str,
    name: &str,
    definition: &RequestTemplate,
) -> Result<UpstreamSavedRequestView, UpstreamWorkspaceError> {
    let endpoint = base_url
        .join(&format!(
            "api/v1/workspaces/{workspace_id}/collections/{collection_id}/requests/{request_id}"
        ))
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    let response = client
        .patch(endpoint)
        .bearer_auth(bearer_token)
        .json(&SaveRequestRequest { name, definition })
        .send()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?;
    parse_workspace_response(response).await
}

pub async fn move_upstream_saved_request(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    collection_id: &str,
    request_id: &str,
    target_collection_id: &str,
) -> Result<UpstreamSavedRequestView, UpstreamWorkspaceError> {
    let endpoint = base_url
        .join(&format!(
            "api/v1/workspaces/{workspace_id}/collections/{collection_id}/requests/{request_id}/collection"
        ))
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    let response = client
        .put(endpoint)
        .bearer_auth(bearer_token)
        .json(&MoveSavedRequestRequest {
            target_collection_id,
        })
        .send()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?;
    parse_workspace_response(response).await
}

pub async fn delete_upstream_saved_request(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    collection_id: &str,
    request_id: &str,
) -> Result<(), UpstreamWorkspaceError> {
    let endpoint = base_url
        .join(&format!(
            "api/v1/workspaces/{workspace_id}/collections/{collection_id}/requests/{request_id}"
        ))
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    let response = client
        .delete(endpoint)
        .bearer_auth(bearer_token)
        .send()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?;
    parse_workspace_response::<serde_json::Value>(response)
        .await
        .map(|_| ())
}

async fn parse_workspace_response<T: for<'de> Deserialize<'de>>(
    mut response: reqwest::Response,
) -> Result<T, UpstreamWorkspaceError> {
    if response.status().is_redirection() {
        return Err(UpstreamWorkspaceError::Redirected);
    }
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > WORKSPACE_RESPONSE_LIMIT_BYTES as u64)
    {
        return Err(UpstreamWorkspaceError::ResponseTooLarge {
            limit_bytes: WORKSPACE_RESPONSE_LIMIT_BYTES,
        });
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(UpstreamWorkspaceError::Transport)?
    {
        if body.len().saturating_add(chunk.len()) > WORKSPACE_RESPONSE_LIMIT_BYTES {
            return Err(UpstreamWorkspaceError::ResponseTooLarge {
                limit_bytes: WORKSPACE_RESPONSE_LIMIT_BYTES,
            });
        }
        body.extend_from_slice(&chunk);
    }
    let envelope: WorkspaceEnvelope<T> = serde_json::from_slice(&body)
        .map_err(|error| UpstreamWorkspaceError::InvalidResponse(error.to_string()))?;
    if !status.is_success() || !envelope.success {
        let message = envelope
            .error
            .map(format_upstream_error)
            .filter(|message| !message.trim().is_empty())
            .unwrap_or_else(|| format!("workspace request failed with HTTP {status}"));
        return Err(UpstreamWorkspaceError::Rejected { status, message });
    }
    envelope.data.ok_or_else(|| {
        UpstreamWorkspaceError::InvalidResponse(
            "the response did not include workspace data".to_owned(),
        )
    })
}

fn flatten_remote_collections(
    collections: Vec<UpstreamCollectionView>,
    parent_folder_id: Option<String>,
    folders: &mut Vec<CollectionFolder>,
    requests: &mut Vec<SavedRequest>,
) {
    for collection in collections {
        let id = collection.id;
        folders.push(CollectionFolder {
            id: id.clone(),
            name: collection.name,
            created_by: collection.created_by.map(Into::into),
            parent_folder_id: parent_folder_id.clone(),
        });
        requests.extend(
            collection
                .requests
                .into_iter()
                .map(|request| request.into_local(Some(id.clone()))),
        );
        flatten_remote_collections(collection.sub_collections, Some(id), folders, requests);
    }
}

fn parse_login_response(
    status: StatusCode,
    body: &[u8],
    base_url: Url,
) -> Result<UpstreamLoginResult, UpstreamLoginError> {
    let envelope: LoginEnvelope = serde_json::from_slice(body)
        .map_err(|error| UpstreamLoginError::InvalidResponse(error.to_string()))?;
    if !status.is_success() || !envelope.success {
        let message = envelope
            .error
            .map(format_upstream_error)
            .filter(|message| !message.trim().is_empty())
            .unwrap_or_else(|| format!("login failed with HTTP {status}"));
        return Err(UpstreamLoginError::Rejected { status, message });
    }

    let data = envelope.data.ok_or_else(|| {
        UpstreamLoginError::InvalidResponse("the response did not include login data".to_owned())
    })?;
    if !data.token_type.eq_ignore_ascii_case("bearer") || data.token.is_empty() {
        return Err(UpstreamLoginError::InvalidResponse(
            "the response did not include a Bearer token".to_owned(),
        ));
    }
    if !data.user.active {
        return Err(UpstreamLoginError::InvalidResponse(
            "the server returned an inactive user".to_owned(),
        ));
    }

    Ok(UpstreamLoginResult {
        base_url,
        token: Zeroizing::new(data.token),
        expires_at: data.expires_at,
        user: data.user,
    })
}

pub fn normalize_upstream_url(value: &str) -> Result<Url, UpstreamUrlError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(UpstreamUrlError::Empty);
    }
    let mut url = Url::parse(value)?;
    if url.host().is_none() {
        return Err(UpstreamUrlError::MissingHost);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(UpstreamUrlError::EmbeddedCredentials);
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(UpstreamUrlError::QueryOrFragment);
    }
    match url.scheme() {
        "https" => {}
        "http" if is_loopback_url(&url) => {}
        "http" => return Err(UpstreamUrlError::InsecureTransport),
        _ => return Err(UpstreamUrlError::UnsupportedScheme),
    }

    if !url.path().ends_with('/') {
        url.path_segments_mut()
            .map_err(|_| UpstreamUrlError::MissingHost)?
            .push("");
    }
    Ok(url)
}

pub fn upstream_url_label(url: &Url) -> String {
    let host = url.host_str().unwrap_or("server");
    let authority = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    let path = url.path().trim_matches('/');
    if path.is_empty() {
        authority
    } else {
        format!("{authority}/{path}")
    }
}

fn is_loopback_url(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
        Some(Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        Some(Host::Domain(domain)) => {
            domain.eq_ignore_ascii_case("localhost")
                || domain.to_ascii_lowercase().ends_with(".localhost")
        }
        None => false,
    }
}

fn new_upstream_id() -> String {
    let sequence = NEXT_UPSTREAM_ID.fetch_add(1, Ordering::Relaxed);
    format!("upstream-{}-{sequence}", Utc::now().timestamp_micros())
}

#[cfg(test)]
mod tests {
    use std::{
        io::Write as _,
        net::{TcpListener, TcpStream},
        thread,
    };

    use super::*;
    use crate::core::{BodyField, HeaderEntry};

    fn upstream_creator(id: &str, display_name: &str) -> UpstreamUserSummary {
        UpstreamUserSummary {
            id: id.to_owned(),
            email: format!("{id}@example.test"),
            display_name: display_name.to_owned(),
        }
    }

    #[test]
    fn upstream_validation_errors_keep_field_reasons() {
        let message = format_upstream_error(LoginErrorBody {
            code: String::new(),
            message: "request validation failed".to_owned(),
            fields: BTreeMap::from([
                ("url".to_owned(), "must use HTTP or HTTPS".to_owned()),
                ("method".to_owned(), "is not a valid HTTP method".to_owned()),
            ]),
        });

        assert_eq!(
            message,
            "request validation failed (method: is not a valid HTTP method, url: must use HTTP or HTTPS)"
        );
    }

    #[test]
    fn normalizes_secure_and_loopback_server_urls() {
        assert_eq!(
            normalize_upstream_url("https://Example.COM/resolved")
                .unwrap()
                .as_str(),
            "https://example.com/resolved/"
        );
        assert_eq!(
            normalize_upstream_url("http://127.0.0.1:8787")
                .unwrap()
                .as_str(),
            "http://127.0.0.1:8787/"
        );
        assert_eq!(
            normalize_upstream_url("http://dev.localhost:8787/")
                .unwrap()
                .as_str(),
            "http://dev.localhost:8787/"
        );
        assert_eq!(
            normalize_upstream_url("https://example.com/tenant%20one")
                .unwrap()
                .as_str(),
            "https://example.com/tenant%20one/"
        );
    }

    #[test]
    fn rejects_unsafe_or_ambiguous_server_urls() {
        assert!(matches!(
            normalize_upstream_url("http://resolved.example.com"),
            Err(UpstreamUrlError::InsecureTransport)
        ));
        assert!(matches!(
            normalize_upstream_url("https://user:password@resolved.example.com"),
            Err(UpstreamUrlError::EmbeddedCredentials)
        ));
        assert!(matches!(
            normalize_upstream_url("https://resolved.example.com?tenant=a"),
            Err(UpstreamUrlError::QueryOrFragment)
        ));
    }

    #[test]
    fn upstream_settings_switch_between_local_and_multiple_servers() {
        let base_a = normalize_upstream_url("https://one.example.com").unwrap();
        let base_b = normalize_upstream_url("https://two.example.com").unwrap();
        let user = LoginUser {
            id: "user-1".to_owned(),
            email: "owner".to_owned(),
            display_name: "Owner".to_owned(),
            active: true,
            roles: Vec::new(),
        };
        let expires = Utc::now() + chrono::Duration::hours(1);
        let first = UpstreamProfile::from_login(None, &base_a, &user, expires);
        let second = UpstreamProfile::from_login(None, &base_b, &user, expires);
        let first_id = first.id.clone();
        let second_id = second.id.clone();
        let mut settings = UpstreamSettings::default();

        settings.upsert(first);
        settings.upsert(second);
        assert!(settings.select(&second_id));
        assert_eq!(
            settings.active().map(|server| server.id.as_str()),
            Some(second_id.as_str())
        );
        assert!(settings.select(&first_id));
        assert_eq!(
            settings.active().map(|server| server.id.as_str()),
            Some(first_id.as_str())
        );
        settings.select_local();
        assert!(settings.active().is_none());
        assert_eq!(settings.servers.len(), 2);
    }

    #[test]
    fn profiles_saved_before_permission_caching_remain_readable() {
        let base_url = normalize_upstream_url("https://one.example.com").unwrap();
        let user = LoginUser {
            id: "user-1".to_owned(),
            email: "owner".to_owned(),
            display_name: "Owner".to_owned(),
            active: true,
            roles: vec![LoginRole {
                permissions: vec![LoginPermission {
                    key: "collections.update".to_owned(),
                }],
            }],
        };
        let profile = UpstreamProfile::from_login(
            None,
            &base_url,
            &user,
            Utc::now() + chrono::Duration::hours(1),
        );
        let mut saved = serde_json::to_value(profile).unwrap();
        saved.as_object_mut().unwrap().remove("permission_keys");

        let restored: UpstreamProfile = serde_json::from_value(saved).unwrap();

        assert!(restored.permission_keys.is_empty());
    }

    #[test]
    fn remembers_an_active_environment_for_each_server_workspace() {
        let base_url = normalize_upstream_url("https://resolved.example.com").unwrap();
        let user = LoginUser {
            id: "user-1".to_owned(),
            email: "owner".to_owned(),
            display_name: "Owner".to_owned(),
            active: true,
            roles: Vec::new(),
        };
        let mut profile = UpstreamProfile::from_login(
            None,
            &base_url,
            &user,
            Utc::now() + chrono::Duration::hours(1),
        );

        profile.set_active_environment_id("workspace-a", Some("environment-a"));
        profile.set_active_environment_id("workspace-b", Some("environment-b"));
        assert_eq!(
            profile.active_environment_id("workspace-a"),
            Some("environment-a")
        );
        assert_eq!(
            profile.active_environment_id("workspace-b"),
            Some("environment-b")
        );

        profile.set_active_environment_id("workspace-a", None);
        assert_eq!(profile.active_environment_id("workspace-a"), None);
        assert_eq!(
            profile.active_environment_id("workspace-b"),
            Some("environment-b")
        );
    }

    #[test]
    fn login_envelope_never_exposes_tokens_through_debug() {
        let result = UpstreamLoginResult {
            base_url: normalize_upstream_url("https://resolved.example.com").unwrap(),
            token: Zeroizing::new("top-secret-token".to_owned()),
            expires_at: Utc::now(),
            user: LoginUser {
                id: "user-1".to_owned(),
                email: "owner".to_owned(),
                display_name: "Owner".to_owned(),
                active: true,
                roles: Vec::new(),
            },
        };
        let debug = format!("{result:?}");
        assert!(!debug.contains("top-secret-token"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn parses_the_server_login_envelope_and_preserves_the_normalized_base() {
        let base_url = normalize_upstream_url("https://resolved.example.com/team").unwrap();
        let expires_at = Utc::now() + chrono::Duration::hours(1);
        let body = serde_json::json!({
            "request_id": "request-1",
            "success": true,
            "data": {
                "token": "server-session-token",
                "token_type": "Bearer",
                "expires_at": expires_at,
                "user": {
                    "id": "user-1",
                    "email": "owner",
                    "display_name": "Owner",
                    "active": true,
                    "roles": [{
                        "permissions": [
                            {"key": "collections.create"},
                            {"key": "requests.read"}
                        ]
                    }]
                }
            }
        });

        let result = parse_login_response(
            StatusCode::OK,
            &serde_json::to_vec(&body).unwrap(),
            base_url.clone(),
        )
        .unwrap();

        assert_eq!(result.base_url, base_url);
        assert_eq!(result.token.as_str(), "server-session-token");
        assert_eq!(result.user.email, "owner");
        assert_eq!(
            result.user.permission_keys(),
            BTreeSet::from(["collections.create".to_owned(), "requests.read".to_owned(),])
        );
        assert_eq!(result.expires_at, expires_at);
    }

    #[test]
    fn exposes_safe_server_rejections_without_echoing_request_credentials() {
        let base_url = normalize_upstream_url("https://resolved.example.com").unwrap();
        let body = br#"{
            "success": false,
            "error": {"code": "invalid_credentials", "message": "login or password is incorrect"}
        }"#;

        let error = parse_login_response(StatusCode::UNAUTHORIZED, body, base_url).unwrap_err();

        assert!(matches!(
            error,
            UpstreamLoginError::Rejected {
                status: StatusCode::UNAUTHORIZED,
                ref message,
            } if message == "login or password is incorrect"
        ));
    }

    #[test]
    fn validation_rejects_duplicate_or_unsafe_persisted_profiles() {
        let base_url = normalize_upstream_url("https://resolved.example.com").unwrap();
        let user = LoginUser {
            id: "user-1".to_owned(),
            email: "owner".to_owned(),
            display_name: "Owner".to_owned(),
            active: true,
            roles: Vec::new(),
        };
        let expires = Utc::now() + chrono::Duration::hours(1);
        let profile = UpstreamProfile::from_login(None, &base_url, &user, expires);
        let mut settings = UpstreamSettings {
            active_upstream_id: Some(profile.id.clone()),
            servers: vec![profile.clone(), profile],
            extra: BTreeMap::new(),
        };
        assert!(settings.validation_warning().is_some());

        settings.servers.pop();
        settings.servers[0].base_url = "http://resolved.example.com/".to_owned();
        assert!(settings.validation_warning().is_some());
    }

    #[test]
    fn maps_recursive_server_collections_and_creator_attribution_to_the_local_tree_shape() {
        let now = Utc::now();
        let view = UpstreamWorkspaceView {
            id: "workspace-1".to_owned(),
            name: "Team API".to_owned(),
            user_ids: vec!["user-1".to_owned()],
            created_by: Some(upstream_creator("workspace-user", "Workspace owner")),
            collections: vec![UpstreamCollectionView {
                id: "root".to_owned(),
                workspace_id: "workspace-1".to_owned(),
                parent_collection_id: None,
                name: "Root".to_owned(),
                user_ids: Vec::new(),
                created_by: Some(upstream_creator("root-user", "Root creator")),
                requests: vec![UpstreamSavedRequestView {
                    id: "root-request".to_owned(),
                    collection_id: "root".to_owned(),
                    name: "Root request".to_owned(),
                    definition: RequestTemplate::default(),
                    created_by: Some(upstream_creator("root-request-user", "Root requester")),
                    created_at: now,
                    updated_at: now,
                }],
                sub_collections: vec![UpstreamCollectionView {
                    id: "child".to_owned(),
                    workspace_id: "workspace-1".to_owned(),
                    parent_collection_id: Some("root".to_owned()),
                    name: "Child".to_owned(),
                    user_ids: Vec::new(),
                    created_by: Some(upstream_creator("child-user", "Child creator")),
                    requests: vec![UpstreamSavedRequestView {
                        id: "child-request".to_owned(),
                        collection_id: "child".to_owned(),
                        name: "Child request".to_owned(),
                        definition: RequestTemplate::default(),
                        created_by: Some(upstream_creator("child-request-user", "Child requester")),
                        created_at: now,
                        updated_at: now,
                    }],
                    sub_collections: vec![UpstreamCollectionView {
                        id: "grandchild".to_owned(),
                        workspace_id: "workspace-1".to_owned(),
                        parent_collection_id: Some("child".to_owned()),
                        name: "Grandchild".to_owned(),
                        user_ids: Vec::new(),
                        created_by: Some(upstream_creator("grandchild-user", "Grandchild creator")),
                        requests: Vec::new(),
                        sub_collections: Vec::new(),
                        created_at: now,
                        updated_at: now,
                    }],
                    created_at: now,
                    updated_at: now,
                }],
                created_at: now,
                updated_at: now,
            }],
            created_at: now,
            updated_at: now,
        };

        let workspace = view.into_local_workspace();

        assert_eq!(
            workspace
                .created_by
                .as_ref()
                .map(|creator| creator.id.as_str()),
            Some("workspace-user")
        );
        assert_eq!(workspace.collections.len(), 1);
        assert_eq!(workspace.collections[0].id, "root");
        assert_eq!(
            workspace.collections[0]
                .created_by
                .as_ref()
                .map(|creator| creator.id.as_str()),
            Some("root-user")
        );
        assert_eq!(workspace.collections[0].folders.len(), 2);
        assert_eq!(workspace.collections[0].folders[0].id, "child");
        assert_eq!(
            workspace.collections[0].folders[0]
                .created_by
                .as_ref()
                .map(|creator| creator.id.as_str()),
            Some("child-user")
        );
        assert_eq!(workspace.collections[0].folders[0].parent_folder_id, None);
        assert_eq!(workspace.collections[0].folders[1].id, "grandchild");
        assert_eq!(
            workspace.collections[0].folders[1]
                .created_by
                .as_ref()
                .map(|creator| creator.id.as_str()),
            Some("grandchild-user")
        );
        assert_eq!(
            workspace.collections[0].folders[1]
                .parent_folder_id
                .as_deref(),
            Some("child")
        );
        assert_eq!(workspace.collections[0].requests.len(), 2);
        assert_eq!(workspace.collections[0].requests[0].id, "root-request");
        assert_eq!(workspace.collections[0].requests[0].folder_id, None);
        assert_eq!(
            workspace.collections[0].requests[0]
                .created_by
                .as_ref()
                .map(|creator| creator.id.as_str()),
            Some("root-request-user")
        );
        assert_eq!(workspace.collections[0].requests[1].id, "child-request");
        assert_eq!(
            workspace.collections[0].requests[1]
                .created_by
                .as_ref()
                .map(|creator| creator.id.as_str()),
            Some("child-request-user")
        );
        assert_eq!(
            workspace.collections[0].requests[1].folder_id.as_deref(),
            Some("child")
        );
        workspace.validate().unwrap();
    }

    #[test]
    fn lists_server_workspaces_with_the_saved_bearer_session() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 2048];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
            }
            let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
            assert!(request.starts_with("get /api/v1/workspaces http/1.1\r\n"));
            assert!(request.contains("authorization: bearer saved-session-token\r\n"));

            let now = Utc::now();
            let body = serde_json::to_vec(&serde_json::json!({
                "request_id": "request-1",
                "success": true,
                "data": [{
                    "id": "workspace-1",
                    "name": "Team API",
                    "user_ids": ["user-1"],
                    "collections": [],
                    "created_at": now,
                    "updated_at": now
                }]
            }))
            .unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        });

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = build_upstream_client().unwrap();
        let base_url = Url::parse(&format!("http://{address}/")).unwrap();
        let workspaces = runtime
            .block_on(list_upstream_workspaces(
                &client,
                &base_url,
                "saved-session-token",
            ))
            .unwrap();

        server.join().unwrap();
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].id, "workspace-1");
        assert_eq!(workspaces[0].name, "Team API");
    }

    #[test]
    fn lists_server_environments_with_the_current_users_values() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let request = read_http_request(&mut stream);
            let headers = String::from_utf8(request).unwrap().to_ascii_lowercase();
            assert!(
                headers.starts_with("get /api/v1/workspaces/workspace-1/environments http/1.1\r\n")
            );
            assert!(headers.contains("authorization: bearer saved-session-token\r\n"));

            let now = Utc::now();
            let body = serde_json::to_vec(&serde_json::json!({
                "request_id": "request-1",
                "success": true,
                "data": [{
                    "id": "environment-1",
                    "workspace_id": "workspace-1",
                    "name": "Production",
                    "created_by": {
                        "id": "environment-user",
                        "email": "environment-user@example.test",
                        "display_name": "Environment creator"
                    },
                    "variables": [{
                        "id": "variable-1",
                        "environment_id": "environment-1",
                        "key": "api_token",
                        "value": "this-users-token",
                        "enabled": true,
                        "secret": true,
                        "created_by": {
                            "id": "variable-user",
                            "email": "variable-user@example.test",
                            "display_name": "Variable creator"
                        },
                        "created_at": now,
                        "updated_at": now
                    }],
                    "created_at": now,
                    "updated_at": now
                }]
            }))
            .unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        });

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = build_upstream_client().unwrap();
        let base_url = Url::parse(&format!("http://{address}/")).unwrap();
        let environments = runtime
            .block_on(list_upstream_environments(
                &client,
                &base_url,
                "saved-session-token",
                "workspace-1",
            ))
            .unwrap();

        server.join().unwrap();
        let environment = environments.into_iter().next().unwrap().into_local();
        assert_eq!(environment.name, "Production");
        assert_eq!(
            environment
                .created_by
                .as_ref()
                .map(|creator| creator.id.as_str()),
            Some("environment-user")
        );
        assert_eq!(environment.variables[0].key, "api_token");
        assert_eq!(environment.variables[0].value, "this-users-token");
        assert!(environment.variables[0].secret);
        assert_eq!(
            environment.variables[0]
                .created_by
                .as_ref()
                .map(|creator| creator.id.as_str()),
            Some("variable-user")
        );
    }

    #[test]
    fn saves_shared_environment_changes_and_personal_values_through_separate_routes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let now = Utc::now();
            for step in 0..5 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                let request = read_http_request(&mut stream);
                let header_end = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .unwrap();
                let headers = String::from_utf8(request[..header_end].to_vec())
                    .unwrap()
                    .to_ascii_lowercase();
                assert!(headers.contains("authorization: bearer saved-session-token\r\n"));
                let request_body = (request.len() > header_end + 4).then(|| {
                    serde_json::from_slice::<serde_json::Value>(&request[header_end + 4..]).unwrap()
                });

                let (expected_start, expected_body, response_data) = match step {
                    0 => (
                        "patch /api/v1/workspaces/workspace-1/environments/environment-1 http/1.1\r\n",
                        Some(serde_json::json!({"name": "Production"})),
                        serde_json::json!({
                            "id": "environment-1",
                            "workspace_id": "workspace-1",
                            "name": "Production",
                            "variables": [],
                            "created_at": now,
                            "updated_at": now
                        }),
                    ),
                    1 => (
                        "delete /api/v1/workspaces/workspace-1/environments/environment-1/variables/variable-removed http/1.1\r\n",
                        None,
                        serde_json::json!({}),
                    ),
                    2 => (
                        "patch /api/v1/workspaces/workspace-1/environments/environment-1/variables/variable-1 http/1.1\r\n",
                        Some(serde_json::json!({
                            "key": "service_token",
                            "enabled": false,
                            "secret": true
                        })),
                        environment_variable_response(
                            "variable-1",
                            "service_token",
                            "old-value",
                            false,
                            true,
                            now,
                        ),
                    ),
                    3 => (
                        "put /api/v1/workspaces/workspace-1/environments/environment-1/variables/variable-1/value http/1.1\r\n",
                        Some(serde_json::json!({"value": "new-value"})),
                        environment_variable_response(
                            "variable-1",
                            "service_token",
                            "new-value",
                            false,
                            true,
                            now,
                        ),
                    ),
                    4 => (
                        "post /api/v1/workspaces/workspace-1/environments/environment-1/variables http/1.1\r\n",
                        Some(serde_json::json!({
                            "key": "base_url",
                            "value": "https://api.example.com",
                            "enabled": true,
                            "secret": false
                        })),
                        environment_variable_response(
                            "variable-new-server-id",
                            "base_url",
                            "https://api.example.com",
                            true,
                            false,
                            now,
                        ),
                    ),
                    _ => unreachable!(),
                };
                assert!(headers.starts_with(expected_start));
                assert_eq!(request_body, expected_body);
                let body = serde_json::to_vec(&serde_json::json!({
                    "request_id": "response-1",
                    "success": true,
                    "data": response_data
                }))
                .unwrap();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
        });

        let baseline = Environment {
            id: "environment-1".to_owned(),
            name: "Staging".to_owned(),
            created_by: None,
            variables: vec![
                EnvironmentVariable {
                    id: "variable-1".to_owned(),
                    key: "api_token".to_owned(),
                    value: "old-value".to_owned(),
                    enabled: true,
                    secret: false,
                    created_by: None,
                },
                EnvironmentVariable {
                    id: "variable-removed".to_owned(),
                    key: "remove_me".to_owned(),
                    value: String::new(),
                    enabled: true,
                    secret: false,
                    created_by: None,
                },
            ],
        };
        let draft = Environment {
            id: "environment-1".to_owned(),
            name: "Production".to_owned(),
            created_by: None,
            variables: vec![
                EnvironmentVariable {
                    id: "variable-1".to_owned(),
                    key: "service_token".to_owned(),
                    value: "new-value".to_owned(),
                    enabled: false,
                    secret: true,
                    created_by: None,
                },
                EnvironmentVariable {
                    id: "draft-variable".to_owned(),
                    key: "base_url".to_owned(),
                    value: "https://api.example.com".to_owned(),
                    enabled: true,
                    secret: false,
                    created_by: None,
                },
            ],
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = build_upstream_client().unwrap();
        let base_url = Url::parse(&format!("http://{address}/")).unwrap();
        runtime
            .block_on(save_upstream_environment(
                &client,
                &base_url,
                "saved-session-token",
                "workspace-1",
                &baseline,
                &draft,
            ))
            .unwrap();

        server.join().unwrap();
    }

    fn environment_variable_response(
        id: &str,
        key: &str,
        value: &str,
        enabled: bool,
        secret: bool,
        now: DateTime<Utc>,
    ) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "environment_id": "environment-1",
            "key": key,
            "value": value,
            "enabled": enabled,
            "secret": secret,
            "created_at": now,
            "updated_at": now
        })
    }

    #[test]
    fn creates_a_server_workspace_with_the_saved_bearer_session() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let request = read_http_request(&mut stream);
            let header_end = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .unwrap();
            let headers = String::from_utf8(request[..header_end].to_vec())
                .unwrap()
                .to_ascii_lowercase();
            assert!(headers.starts_with("post /api/v1/workspaces http/1.1\r\n"));
            assert!(headers.contains("authorization: bearer saved-session-token\r\n"));
            let body: serde_json::Value =
                serde_json::from_slice(&request[header_end + 4..]).unwrap();
            assert_eq!(body, serde_json::json!({"name": "Team API"}));

            let now = Utc::now();
            let body = serde_json::to_vec(&serde_json::json!({
                "request_id": "request-1",
                "success": true,
                "data": {
                    "id": "workspace-1",
                    "name": "Team API",
                    "user_ids": ["user-1"],
                    "collections": [],
                    "created_at": now,
                    "updated_at": now
                }
            }))
            .unwrap();
            write!(
                stream,
                "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        });

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = build_upstream_client().unwrap();
        let base_url = Url::parse(&format!("http://{address}/")).unwrap();
        let workspace = runtime
            .block_on(create_upstream_workspace(
                &client,
                &base_url,
                "saved-session-token",
                "Team API",
            ))
            .unwrap();

        server.join().unwrap();
        assert_eq!(workspace.id, "workspace-1");
        assert_eq!(workspace.name, "Team API");
    }

    #[test]
    fn creates_a_server_collection_with_the_saved_bearer_session() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let request = read_http_request(&mut stream);
            let header_end = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .unwrap();
            let headers = String::from_utf8(request[..header_end].to_vec())
                .unwrap()
                .to_ascii_lowercase();
            assert!(
                headers.starts_with("post /api/v1/workspaces/workspace-1/collections http/1.1\r\n")
            );
            assert!(headers.contains("authorization: bearer saved-session-token\r\n"));
            let body: serde_json::Value =
                serde_json::from_slice(&request[header_end + 4..]).unwrap();
            assert_eq!(
                body,
                serde_json::json!({
                    "name": "Accounts",
                    "parent_collection_id": "collection-root"
                })
            );

            let now = Utc::now();
            let body = serde_json::to_vec(&serde_json::json!({
                "request_id": "request-1",
                "success": true,
                "data": {
                    "id": "collection-child",
                    "workspace_id": "workspace-1",
                    "parent_collection_id": "collection-root",
                    "name": "Accounts",
                    "user_ids": [],
                    "sub_collections": [],
                    "requests": [],
                    "created_at": now,
                    "updated_at": now
                }
            }))
            .unwrap();
            write!(
                stream,
                "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        });

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = build_upstream_client().unwrap();
        let base_url = Url::parse(&format!("http://{address}/")).unwrap();
        let collection = runtime
            .block_on(create_upstream_collection(
                &client,
                &base_url,
                "saved-session-token",
                "workspace-1",
                "Accounts",
                Some("collection-root"),
            ))
            .unwrap();

        server.join().unwrap();
        assert_eq!(collection.id, "collection-child");
        assert_eq!(
            collection.parent_collection_id.as_deref(),
            Some("collection-root")
        );
    }

    #[test]
    fn creates_and_updates_a_server_saved_request() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let definition = RequestTemplate::default();
        let response_definition = serde_json::to_value(&definition).unwrap();
        let server = thread::spawn(move || {
            for expected_method in ["post", "patch"] {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                let request = read_http_request(&mut stream);
                let header_end = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .unwrap();
                let headers = String::from_utf8(request[..header_end].to_vec())
                    .unwrap()
                    .to_ascii_lowercase();
                let expected_path = if expected_method == "post" {
                    "post /api/v1/workspaces/workspace-1/collections/collection-1/requests http/1.1\r\n"
                } else {
                    "patch /api/v1/workspaces/workspace-1/collections/collection-1/requests/request-1 http/1.1\r\n"
                };
                assert!(headers.starts_with(expected_path));
                assert!(headers.contains("authorization: bearer saved-session-token\r\n"));
                let request_body: serde_json::Value =
                    serde_json::from_slice(&request[header_end + 4..]).unwrap();
                let expected_name = if expected_method == "post" {
                    "List users"
                } else {
                    "List all users"
                };
                assert_eq!(request_body["name"], expected_name);
                assert_eq!(request_body["definition"], response_definition);

                let now = Utc::now();
                let body = serde_json::to_vec(&serde_json::json!({
                    "request_id": "response-1",
                    "success": true,
                    "data": {
                        "id": "request-1",
                        "collection_id": "collection-1",
                        "name": expected_name,
                        "definition": response_definition,
                        "created_at": now,
                        "updated_at": now
                    }
                }))
                .unwrap();
                let status = if expected_method == "post" {
                    "201 Created"
                } else {
                    "200 OK"
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
        });

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = build_upstream_client().unwrap();
        let base_url = Url::parse(&format!("http://{address}/")).unwrap();
        let created = runtime
            .block_on(create_upstream_saved_request(
                &client,
                &base_url,
                "saved-session-token",
                "workspace-1",
                "collection-1",
                "List users",
                &definition,
            ))
            .unwrap();
        let updated = runtime
            .block_on(update_upstream_saved_request(
                &client,
                &base_url,
                "saved-session-token",
                "workspace-1",
                "collection-1",
                "request-1",
                "List all users",
                &definition,
            ))
            .unwrap();

        server.join().unwrap();
        assert_eq!(created.id, "request-1");
        assert_eq!(updated.name, "List all users");
    }

    #[test]
    fn remote_crud_helpers_use_the_server_contract() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let now = Utc::now();
        let definition = RequestTemplate::default();
        let response_definition = serde_json::to_value(&definition).unwrap();
        let workspace_response = serde_json::json!({
            "id": "workspace-1",
            "name": "Renamed workspace",
            "user_ids": ["user-1"],
            "collections": [],
            "created_at": now,
            "updated_at": now
        });
        let collection_response = serde_json::json!({
            "id": "collection-1",
            "workspace_id": "workspace-1",
            "parent_collection_id": "collection-root",
            "name": "Renamed collection",
            "user_ids": [],
            "sub_collections": [],
            "requests": [],
            "created_at": now,
            "updated_at": now
        });
        let moved_request_response = serde_json::json!({
            "id": "request-1",
            "collection_id": "collection-2",
            "name": "List users",
            "definition": response_definition,
            "created_at": now,
            "updated_at": now
        });
        let expected = vec![
            (
                "get /api/v1/auth/me http/1.1\r\n",
                None,
                serde_json::json!({
                    "id": "user-1",
                    "email": "owner",
                    "display_name": "Owner",
                    "active": true,
                    "roles": [{"permissions": [{"key": "collections.update"}]}]
                }),
            ),
            (
                "get /api/v1/workspaces/workspace-1 http/1.1\r\n",
                None,
                workspace_response.clone(),
            ),
            (
                "patch /api/v1/workspaces/workspace-1 http/1.1\r\n",
                Some(serde_json::json!({"name": "Renamed workspace"})),
                workspace_response,
            ),
            (
                "patch /api/v1/workspaces/workspace-1/collections/collection-1 http/1.1\r\n",
                Some(serde_json::json!({"name": "Renamed collection"})),
                collection_response.clone(),
            ),
            (
                "put /api/v1/workspaces/workspace-1/collections/collection-1/parent http/1.1\r\n",
                Some(serde_json::json!({
                    "parent_collection_id": "collection-root"
                })),
                collection_response,
            ),
            (
                "put /api/v1/workspaces/workspace-1/collections/collection-1/requests/request-1/collection http/1.1\r\n",
                Some(serde_json::json!({"target_collection_id": "collection-2"})),
                moved_request_response,
            ),
            (
                "delete /api/v1/workspaces/workspace-1/collections/collection-2/requests/request-1 http/1.1\r\n",
                None,
                serde_json::json!({}),
            ),
            (
                "delete /api/v1/workspaces/workspace-1/collections/collection-1 http/1.1\r\n",
                None,
                serde_json::json!({}),
            ),
            (
                "delete /api/v1/workspaces/workspace-1 http/1.1\r\n",
                None,
                serde_json::json!({}),
            ),
        ];
        let server = thread::spawn(move || {
            for (expected_start, expected_body, response_data) in expected {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                let request = read_http_request(&mut stream);
                let header_end = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .unwrap();
                let headers = String::from_utf8(request[..header_end].to_vec())
                    .unwrap()
                    .to_ascii_lowercase();
                assert!(headers.starts_with(expected_start));
                assert!(headers.contains("authorization: bearer saved-session-token\r\n"));
                match expected_body {
                    Some(expected_body) => {
                        let body: serde_json::Value =
                            serde_json::from_slice(&request[header_end + 4..]).unwrap();
                        assert_eq!(body, expected_body);
                    }
                    None => assert_eq!(request.len(), header_end + 4),
                }

                let body = serde_json::to_vec(&serde_json::json!({
                    "request_id": "response-1",
                    "success": true,
                    "data": response_data
                }))
                .unwrap();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
        });

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = build_upstream_client().unwrap();
        let base_url = Url::parse(&format!("http://{address}/")).unwrap();
        let user = runtime
            .block_on(get_upstream_user(&client, &base_url, "saved-session-token"))
            .unwrap();
        assert!(user.permission_keys().contains("collections.update"));
        runtime
            .block_on(get_upstream_workspace(
                &client,
                &base_url,
                "saved-session-token",
                "workspace-1",
            ))
            .unwrap();
        runtime
            .block_on(update_upstream_workspace(
                &client,
                &base_url,
                "saved-session-token",
                "workspace-1",
                "Renamed workspace",
            ))
            .unwrap();
        runtime
            .block_on(update_upstream_collection(
                &client,
                &base_url,
                "saved-session-token",
                "workspace-1",
                "collection-1",
                "Renamed collection",
            ))
            .unwrap();
        runtime
            .block_on(move_upstream_collection(
                &client,
                &base_url,
                "saved-session-token",
                "workspace-1",
                "collection-1",
                Some("collection-root"),
            ))
            .unwrap();
        let moved = runtime
            .block_on(move_upstream_saved_request(
                &client,
                &base_url,
                "saved-session-token",
                "workspace-1",
                "collection-1",
                "request-1",
                "collection-2",
            ))
            .unwrap();
        assert_eq!(moved.collection_id, "collection-2");
        runtime
            .block_on(delete_upstream_saved_request(
                &client,
                &base_url,
                "saved-session-token",
                "workspace-1",
                "collection-2",
                "request-1",
            ))
            .unwrap();
        runtime
            .block_on(delete_upstream_collection(
                &client,
                &base_url,
                "saved-session-token",
                "workspace-1",
                "collection-1",
            ))
            .unwrap();
        runtime
            .block_on(delete_upstream_workspace(
                &client,
                &base_url,
                "saved-session-token",
                "workspace-1",
            ))
            .unwrap();

        server.join().unwrap();
    }

    #[test]
    fn executes_a_request_through_the_server_contract_and_decodes_binary_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut policy_stream, _) = listener.accept().unwrap();
            let policy_request = read_http_request(&mut policy_stream);
            let policy_header_end = policy_request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .unwrap();
            let policy_headers =
                String::from_utf8(policy_request[..policy_header_end].to_vec()).unwrap();
            assert!(policy_headers.starts_with("GET /api/v1/request-execution HTTP/1.1\r\n"));
            let policy_body = br#"{"success":true,"data":{"mode":"server"}}"#;
            write!(
                policy_stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                policy_body.len()
            )
            .unwrap();
            policy_stream.write_all(policy_body).unwrap();

            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let request = read_http_request(&mut stream);
            let header_end = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .unwrap();
            let headers = String::from_utf8(request[..header_end].to_vec()).unwrap();
            assert!(
                headers.starts_with("POST /api/v1/workspaces/workspace-1/execute HTTP/1.1\r\n")
            );
            assert!(headers.contains("authorization: Bearer saved-session-token\r\n"));
            let body: serde_json::Value =
                serde_json::from_slice(&request[header_end + 4..]).unwrap();
            assert_eq!(body["method"], "PATCH");
            assert_eq!(body["url"], "https://target.example.test/items/42");
            assert_eq!(body["body"]["mode"], "raw");
            assert_eq!(
                BASE64_STANDARD
                    .decode(body["body"]["data_base64"].as_str().unwrap())
                    .unwrap(),
                br#"{"active":true}"#
            );
            assert!(
                body["headers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|header| { header["name"] == "X-Test" && header["value"] == "yes" })
            );
            assert!(body["headers"].as_array().unwrap().iter().any(|header| {
                header["name"] == "User-Agent"
                    && header["value"] == concat!("resolved/", env!("RESOLVED_BUILD_VERSION"))
            }));

            let response_body = serde_json::to_vec(&serde_json::json!({
                "request_id": "response-1",
                "success": true,
                "data": {
                    "status": 202,
                    "status_text": "Accepted",
                    "http_version": "HTTP/2.0",
                    "final_url": "https://target.example.test/items/42",
                    "headers": [
                        {"name": "Content-Type", "value": "application/octet-stream"},
                        {"name": "X-Target", "value": "proxied"}
                    ],
                    "content_type": "application/octet-stream",
                    "body_base64": BASE64_STANDARD.encode([0_u8, 1, 2, 255]),
                    "duration_micros": 1250
                }
            }))
            .unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response_body.len()
            )
            .unwrap();
            stream.write_all(&response_body).unwrap();
        });

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = build_upstream_execution_client().unwrap();
        let local_client = super::super::request::build_client().unwrap();
        let mut request = RequestDraft::new("PATCH", "https://target.example.test/items/42");
        request.headers = vec![HeaderEntry::new("X-Test", "yes")];
        request.body = r#"{"active":true}"#.to_owned();
        let response = runtime
            .block_on(send_request_for_upstream_workspace(
                &client,
                &local_client,
                &Url::parse(&format!("http://{address}/")).unwrap(),
                "saved-session-token",
                "workspace-1",
                request,
            ))
            .unwrap();

        server.join().unwrap();
        assert_eq!(response.status, 202);
        assert_eq!(response.status_text, "Accepted");
        assert_eq!(response.http_version, "HTTP/2.0");
        assert_eq!(response.body.as_ref(), &[0, 1, 2, 255]);
        assert_eq!(response.duration, Duration::from_micros(1250));
        assert_eq!(
            response.content_type.as_deref(),
            Some("application/octet-stream")
        );
    }

    #[test]
    fn proxied_request_validation_error_includes_the_field_reason() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_http_request(&mut stream);
            let response_body = br#"{
                "success": false,
                "error": {
                    "code": "validation_failed",
                    "message": "request validation failed",
                    "fields": {"url": "must use HTTP or HTTPS"}
                }
            }"#;
            write!(
                stream,
                "HTTP/1.1 422 Unprocessable Entity\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response_body.len()
            )
            .unwrap();
            stream.write_all(response_body).unwrap();
        });

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = build_upstream_execution_client().unwrap();
        let error = runtime
            .block_on(execute_upstream_request(
                &client,
                &Url::parse(&format!("http://{address}/")).unwrap(),
                "saved-session-token",
                "workspace-1",
                RequestDraft::new("GET", "https://target.example.test"),
            ))
            .unwrap_err();

        server.join().unwrap();
        assert_eq!(
            error.to_string(),
            "request validation failed (url: must use HTTP or HTTPS)"
        );
    }

    #[test]
    fn proxied_blocked_destination_error_remains_structured() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_http_request(&mut stream);
            let response_body = br#"{
                "success": false,
                "error": {
                    "code": "proxy_destination_blocked",
                    "message": "the proxied request was blocked",
                    "fields": {
                        "request": "http://127.0.0.1/private",
                        "address": "127.0.0.1",
                        "reason": "loopback addresses"
                    }
                }
            }"#;
            write!(
                stream,
                "HTTP/1.1 422 Unprocessable Entity\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response_body.len()
            )
            .unwrap();
            stream.write_all(response_body).unwrap();
        });

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = build_upstream_execution_client().unwrap();
        let error = runtime
            .block_on(execute_upstream_request(
                &client,
                &Url::parse(&format!("http://{address}/")).unwrap(),
                "saved-session-token",
                "workspace-1",
                RequestDraft::new("GET", "https://target.example.test"),
            ))
            .unwrap_err();

        server.join().unwrap();
        match error {
            RequestError::ProxyDestinationBlocked {
                request,
                address,
                reason,
            } => {
                assert_eq!(request, "http://127.0.0.1/private");
                assert_eq!(address, "127.0.0.1");
                assert_eq!(reason, "loopback addresses");
            }
            other => panic!("error = {other:?}"),
        }
    }

    #[test]
    fn local_server_policy_keeps_target_execution_on_the_desktop() {
        let policy_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let policy_address = policy_listener.local_addr().unwrap();
        let policy_server = thread::spawn(move || {
            let (mut stream, _) = policy_listener.accept().unwrap();
            let request = read_http_request(&mut stream);
            let header_end = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .unwrap();
            let headers = String::from_utf8(request[..header_end].to_vec()).unwrap();
            assert!(headers.starts_with("GET /api/v1/request-execution HTTP/1.1\r\n"));
            assert!(headers.contains("authorization: Bearer saved-session-token\r\n"));
            let response_body = br#"{"success":true,"data":{"mode":"local"}}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response_body.len()
            )
            .unwrap();
            stream.write_all(response_body).unwrap();
        });

        let target_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let target_address = target_listener.local_addr().unwrap();
        let target_server = thread::spawn(move || {
            let (mut stream, _) = target_listener.accept().unwrap();
            let request = read_http_request(&mut stream);
            let header_end = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .unwrap();
            let headers = String::from_utf8(request[..header_end].to_vec()).unwrap();
            assert!(headers.starts_with("GET /from-desktop HTTP/1.1\r\n"));
            let response_body = b"local response";
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response_body.len()
            )
            .unwrap();
            stream.write_all(response_body).unwrap();
        });

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let upstream_client = build_upstream_execution_client().unwrap();
        let local_client = super::super::request::build_client().unwrap();
        let response = runtime
            .block_on(send_request_for_upstream_workspace(
                &upstream_client,
                &local_client,
                &Url::parse(&format!("http://{policy_address}/")).unwrap(),
                "saved-session-token",
                "workspace-1",
                RequestDraft::new("GET", format!("http://{target_address}/from-desktop")),
            ))
            .unwrap();

        policy_server.join().unwrap();
        target_server.join().unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body.as_ref(), b"local response");
    }

    #[test]
    fn missing_policy_route_keeps_older_servers_on_local_execution() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_http_request(&mut stream);
            let header_end = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .unwrap();
            let headers = String::from_utf8(request[..header_end].to_vec()).unwrap();
            assert!(headers.starts_with("GET /api/v1/request-execution HTTP/1.1\r\n"));
            let response_body = br#"{"success":false,"error":{"message":"not found"}}"#;
            write!(
                stream,
                "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response_body.len()
            )
            .unwrap();
            stream.write_all(response_body).unwrap();
        });

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mode = runtime
            .block_on(get_upstream_execution_policy(
                &build_upstream_execution_client().unwrap(),
                &Url::parse(&format!("http://{address}/")).unwrap(),
                "saved-session-token",
            ))
            .unwrap();

        server.join().unwrap();
        assert_eq!(mode, RequestExecutionMode::Local);
    }

    #[test]
    fn multipart_proxy_payload_reads_file_bytes_without_sending_the_local_path() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"private fixture bytes").unwrap();
        let path = file.path().to_string_lossy().into_owned();
        let mut request = RequestDraft::new("POST", "https://target.example.test/upload");
        request.body_mode = BodyMode::MultipartFormData;
        request.body_fields = vec![
            BodyField::text("description", "fixture"),
            BodyField::file("document", path.clone()),
        ];
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let payload = runtime.block_on(proxy_request_payload(request)).unwrap();
        let encoded = serde_json::to_value(payload).unwrap();

        assert_eq!(encoded["body"]["mode"], "multipart_form_data");
        assert_eq!(encoded["body"]["fields"][0]["value"], "fixture");
        assert_eq!(encoded["body"]["fields"][1]["kind"], "file");
        assert_eq!(
            BASE64_STANDARD
                .decode(
                    encoded["body"]["fields"][1]["content_base64"]
                        .as_str()
                        .unwrap()
                )
                .unwrap(),
            b"private fixture bytes"
        );
        assert!(!encoded.to_string().contains(&path));
    }

    fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 2048];
        let (header_end, content_length) = loop {
            let count = stream.read(&mut buffer).unwrap();
            if count == 0 {
                panic!("connection closed before request headers were complete");
            }
            request.extend_from_slice(&buffer[..count]);
            let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
            else {
                continue;
            };
            let headers = String::from_utf8(request[..header_end].to_vec()).unwrap();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            break (header_end, content_length);
        };
        let expected_length = header_end + 4 + content_length;
        while request.len() < expected_length {
            let count = stream.read(&mut buffer).unwrap();
            if count == 0 {
                panic!("connection closed before request body was complete");
            }
            request.extend_from_slice(&buffer[..count]);
        }
        request.truncate(expected_length);
        request
    }

    #[test]
    fn save_upstream_environment_classifies_and_orders_the_sync() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let now = Utc::now();

        let server = thread::spawn(move || {
            let mut recorded = Vec::new();
            for _ in 0..5 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                let request = read_http_request(&mut stream);
                let header_end = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .unwrap();
                let headers = String::from_utf8(request[..header_end].to_vec()).unwrap();
                let mut parts = headers.lines().next().unwrap().split_whitespace();
                let method = parts.next().unwrap();
                let path = parts.next().unwrap();
                recorded.push(format!("{method} {path}"));

                let body = if method == "DELETE" {
                    serde_json::json!({ "success": true, "data": {} })
                } else if path.ends_with("/environments/env-1") {
                    serde_json::json!({
                        "success": true,
                        "data": {
                            "id": "env-1",
                            "workspace_id": "ws-1",
                            "name": "New",
                            "variables": [],
                            "created_by": null,
                            "created_at": now,
                            "updated_at": now,
                        }
                    })
                } else {
                    serde_json::json!({
                        "success": true,
                        "data": {
                            "id": "variable",
                            "environment_id": "env-1",
                            "key": "key",
                            "value": "value",
                            "enabled": true,
                            "secret": false,
                            "created_by": null,
                            "created_at": now,
                            "updated_at": now,
                        }
                    })
                };
                let body = serde_json::to_vec(&body).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
            recorded
        });

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = build_upstream_client().unwrap();
        let base_url = Url::parse(&format!("http://{address}/")).unwrap();

        fn variable(id: &str, key: &str, value: &str) -> EnvironmentVariable {
            EnvironmentVariable {
                id: id.to_owned(),
                key: key.to_owned(),
                value: value.to_owned(),
                enabled: true,
                secret: false,
                created_by: None,
            }
        }

        let baseline = Environment {
            id: "env-1".to_owned(),
            name: "Old".to_owned(),
            created_by: None,
            variables: vec![
                variable("v1", "keep", "one"),
                variable("v2", "rename", "two"),
                variable("v3", "gone", "three"),
            ],
        };
        let draft = Environment {
            id: "env-1".to_owned(),
            name: "New".to_owned(),
            created_by: None,
            variables: vec![
                variable("v2", "rename", "two-changed"),
                variable("v3", "renamed-key", "three"),
                variable("v4", "brand-new", "four"),
            ],
        };

        runtime
            .block_on(save_upstream_environment(
                &client,
                &base_url,
                "saved-session-token",
                "ws-1",
                &baseline,
                &draft,
            ))
            .unwrap();

        let recorded = server.join().unwrap();
        assert_eq!(
            recorded,
            vec![
                "PATCH /api/v1/workspaces/ws-1/environments/env-1".to_owned(),
                "DELETE /api/v1/workspaces/ws-1/environments/env-1/variables/v1".to_owned(),
                "PUT /api/v1/workspaces/ws-1/environments/env-1/variables/v2/value".to_owned(),
                "PATCH /api/v1/workspaces/ws-1/environments/env-1/variables/v3".to_owned(),
                "POST /api/v1/workspaces/ws-1/environments/env-1/variables".to_owned(),
            ]
        );
    }
}
