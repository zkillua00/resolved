use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    net::IpAddr,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use chrono::{DateTime, Utc};
use reqwest::{Client, StatusCode, redirect::Policy};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::{Host, Url};
use zeroize::Zeroizing;

use super::{Collection, Workspace, workspace::CollectionFolder};

const LOGIN_RESPONSE_LIMIT_BYTES: usize = 64 * 1024;
const WORKSPACE_RESPONSE_LIMIT_BYTES: usize = 2 * 1024 * 1024;
const LOGIN_TIMEOUT: Duration = Duration::from_secs(20);
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
    pub workspaces: Vec<UpstreamWorkspaceSummary>,
    #[serde(default)]
    pub active_workspace_id: Option<String>,
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
            workspaces: Vec::new(),
            active_workspace_id: None,
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

    pub fn active_workspace(&self) -> Option<&UpstreamWorkspaceSummary> {
        self.active_workspace_id
            .as_deref()
            .and_then(|id| self.workspaces.iter().find(|workspace| workspace.id == id))
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
pub struct UpstreamWorkspaceView {
    pub id: String,
    pub name: String,
    pub user_ids: Vec<String>,
    pub collections: Vec<UpstreamCollectionView>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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
                flatten_remote_collections(root.sub_collections, None, &mut folders);
                Collection {
                    id: root.id,
                    name: root.name,
                    folders,
                    requests: Vec::new(),
                }
            })
            .collect();
        Workspace {
            collections,
            environments: Vec::new(),
            snippets: Vec::new(),
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
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct LoginUser {
    pub id: String,
    pub email: String,
    pub display_name: String,
    pub active: bool,
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
    message: String,
}

#[derive(Deserialize)]
struct WorkspaceEnvelope<T> {
    success: bool,
    data: Option<T>,
    error: Option<LoginErrorBody>,
}

pub fn build_upstream_client() -> Result<Client, UpstreamLoginError> {
    Client::builder()
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION")
        ))
        // Never replay a login/password body to a redirect target.
        .redirect(Policy::none())
        .timeout(LOGIN_TIMEOUT)
        .build()
        .map_err(UpstreamLoginError::Client)
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
            .map(|error| error.message)
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
) {
    for collection in collections {
        let id = collection.id;
        folders.push(CollectionFolder {
            id: id.clone(),
            name: collection.name,
            parent_folder_id: parent_folder_id.clone(),
        });
        flatten_remote_collections(collection.sub_collections, Some(id), folders);
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
            .map(|error| error.message)
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
        io::{Read as _, Write as _},
        net::{TcpListener, TcpStream},
        thread,
    };

    use super::*;

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
                    "roles": []
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
    fn maps_recursive_server_collections_to_the_local_tree_shape() {
        let now = Utc::now();
        let view = UpstreamWorkspaceView {
            id: "workspace-1".to_owned(),
            name: "Team API".to_owned(),
            user_ids: vec!["user-1".to_owned()],
            collections: vec![UpstreamCollectionView {
                id: "root".to_owned(),
                workspace_id: "workspace-1".to_owned(),
                parent_collection_id: None,
                name: "Root".to_owned(),
                user_ids: Vec::new(),
                sub_collections: vec![UpstreamCollectionView {
                    id: "child".to_owned(),
                    workspace_id: "workspace-1".to_owned(),
                    parent_collection_id: Some("root".to_owned()),
                    name: "Child".to_owned(),
                    user_ids: Vec::new(),
                    sub_collections: vec![UpstreamCollectionView {
                        id: "grandchild".to_owned(),
                        workspace_id: "workspace-1".to_owned(),
                        parent_collection_id: Some("child".to_owned()),
                        name: "Grandchild".to_owned(),
                        user_ids: Vec::new(),
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

        assert_eq!(workspace.collections.len(), 1);
        assert_eq!(workspace.collections[0].id, "root");
        assert_eq!(workspace.collections[0].folders.len(), 2);
        assert_eq!(workspace.collections[0].folders[0].id, "child");
        assert_eq!(workspace.collections[0].folders[0].parent_folder_id, None);
        assert_eq!(workspace.collections[0].folders[1].id, "grandchild");
        assert_eq!(
            workspace.collections[0].folders[1]
                .parent_folder_id
                .as_deref(),
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
}
