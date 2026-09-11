use std::collections::BTreeMap;

#[cfg(test)]
use std::collections::BTreeSet;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use chrono::{DateTime, Utc};
use reqwest::{Client, Method, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;
use url::Url;

use super::history::{request_for_shared_history, response_for_shared_history};
use super::{BodyFieldKind, RequestDraft, ResponseData};
use super::{UpstreamCollectionView, UpstreamUserSummary, UpstreamWorkspaceView};

const MANAGEMENT_RESPONSE_LIMIT_BYTES: usize = 32 * 1024 * 1024;

pub const USERS_READ: &str = "users.read";
pub const USERS_CREATE: &str = "users.create";
pub const USERS_UPDATE: &str = "users.update";
pub const USERS_ASSIGN_ROLES: &str = "users.roles.assign";
pub const ROLES_READ: &str = "roles.read";
pub const ROLES_CREATE: &str = "roles.create";
pub const ROLES_UPDATE: &str = "roles.update";
pub const ROLES_ASSIGN_PERMISSIONS: &str = "roles.permissions.assign";
pub const PERMISSIONS_READ: &str = "permissions.read";
pub const WORKSPACES_READ: &str = "workspaces.read";
pub const WORKSPACES_CREATE: &str = "workspaces.create";
pub const WORKSPACES_UPDATE: &str = "workspaces.update";
pub const WORKSPACES_DELETE: &str = "workspaces.delete";
pub const WORKSPACES_ASSIGN_USERS: &str = "workspaces.users.assign";
pub const COLLECTIONS_READ: &str = "collections.read";
pub const COLLECTIONS_CREATE: &str = "collections.create";
pub const COLLECTIONS_UPDATE: &str = "collections.update";
pub const COLLECTIONS_DELETE: &str = "collections.delete";
pub const COLLECTIONS_ASSIGN_USERS: &str = "collections.users.assign";
pub const REQUESTS_READ: &str = "requests.read";
pub const REQUESTS_CREATE: &str = "requests.create";
pub const REQUESTS_UPDATE: &str = "requests.update";
pub const REQUESTS_DELETE: &str = "requests.delete";
pub const SERVER_SETTINGS_READ: &str = "server_settings.read";
pub const SERVER_SETTINGS_UPDATE: &str = "server_settings.update";
pub const HISTORY_READ_OTHERS: &str = "history.read_others";
pub const AUDIT_READ: &str = "audit.read";
pub const ENVIRONMENTS_READ: &str = "environments.read";
pub const ENVIRONMENTS_CREATE: &str = "environments.create";
pub const ENVIRONMENTS_UPDATE: &str = "environments.update";
pub const ENVIRONMENTS_DELETE: &str = "environments.delete";
pub const ENVIRONMENT_VALUES_UPDATE: &str = "environment_values.update";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ExecutionLimitScope {
    #[default]
    Deployment,
    Workspace(String),
    Collection {
        workspace_id: String,
        collection_id: String,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ExecutionLimitSource {
    pub kind: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub collection_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ExecutionLimitDefinition {
    pub key: String,
    pub label: String,
    pub unit: String,
    pub default: super::execution_limits::Bound,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ExecutionLimitSnapshot {
    pub overrides: BTreeMap<String, super::execution_limits::Bound>,
    pub effective: BTreeMap<String, super::execution_limits::Bound>,
    pub sources: BTreeMap<String, ExecutionLimitSource>,
    pub definitions: Vec<ExecutionLimitDefinition>,
}

fn execution_limits_endpoint(
    base_url: &Url,
    scope: &ExecutionLimitScope,
) -> Result<Url, UpstreamManagementError> {
    let mut url = endpoint(base_url, "api/v1/request-execution/limits")?;
    match scope {
        ExecutionLimitScope::Deployment => {}
        ExecutionLimitScope::Workspace(id) => {
            url.query_pairs_mut().append_pair("workspace_id", id);
        }
        ExecutionLimitScope::Collection {
            workspace_id,
            collection_id,
        } => {
            url.query_pairs_mut()
                .append_pair("workspace_id", workspace_id)
                .append_pair("collection_id", collection_id);
        }
    }
    Ok(url)
}

pub async fn load_execution_limits(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    scope: &ExecutionLimitScope,
) -> Result<ExecutionLimitSnapshot, UpstreamManagementError> {
    let response = client
        .get(execution_limits_endpoint(base_url, scope)?)
        .bearer_auth(bearer_token)
        .send()
        .await
        .map_err(UpstreamManagementError::Transport)?;
    parse_response(response).await
}

/// Replace only the selected layer. Missing keys inherit; zero is an explicit bound.
pub async fn replace_execution_limits(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    scope: &ExecutionLimitScope,
    overrides: &BTreeMap<String, super::execution_limits::Bound>,
) -> Result<ExecutionLimitSnapshot, UpstreamManagementError> {
    #[derive(Serialize)]
    struct Replace<'a> {
        overrides: &'a BTreeMap<String, super::execution_limits::Bound>,
    }
    let response = client
        .put(execution_limits_endpoint(base_url, scope)?)
        .bearer_auth(bearer_token)
        .json(&Replace { overrides })
        .send()
        .await
        .map_err(UpstreamManagementError::Transport)?;
    parse_response(response).await
}

#[cfg(test)]
mod execution_limit_tests {
    use super::*;

    #[test]
    fn scope_queries_are_encoded_and_deployment_has_no_scope() {
        let base = Url::parse("https://example.com/").unwrap();
        let deployment =
            execution_limits_endpoint(&base, &ExecutionLimitScope::Deployment).unwrap();
        assert_eq!(deployment.path(), "/api/v1/request-execution/limits");
        assert_eq!(deployment.query(), None);
        let collection = execution_limits_endpoint(
            &base,
            &ExecutionLimitScope::Collection {
                workspace_id: "workspace & one".to_owned(),
                collection_id: "collection/two".to_owned(),
            },
        )
        .unwrap();
        assert_eq!(
            collection.query_pairs().collect::<BTreeMap<_, _>>(),
            BTreeMap::from([
                ("workspace_id".into(), "workspace & one".into()),
                ("collection_id".into(), "collection/two".into()),
            ]),
        );
    }

    #[test]
    fn server_metadata_and_explicit_bounds_are_preserved() {
        let snapshot: ExecutionLimitSnapshot = serde_json::from_value(serde_json::json!({
            "overrides": {
                "future_limit": {"unlimited": false, "value": 0},
                "another_limit": {"unlimited": true, "value": 0}
            },
            "effective": {"future_limit": {"unlimited": false, "value": 0}},
            "sources": {"future_limit": {"kind": "collection", "workspace_id": "w", "collection_id": "c"}},
            "definitions": [{
                "key": "future_limit", "label": "A future server setting", "unit": "count",
                "default": {"unlimited": false, "value": 12}
            }]
        })).unwrap();
        assert_eq!(snapshot.definitions[0].label, "A future server setting");
        assert_eq!(snapshot.overrides["future_limit"].value, 0);
        assert!(!snapshot.overrides["future_limit"].unlimited);
        assert!(snapshot.overrides["another_limit"].unlimited);
        assert_eq!(
            snapshot.sources["future_limit"].collection_id.as_deref(),
            Some("c")
        );
        assert!(!snapshot.overrides.contains_key("inherited_limit"));
        let encoded = serde_json::to_value(&snapshot.overrides).unwrap();
        assert_eq!(
            encoded["future_limit"],
            serde_json::json!({"unlimited": false, "value": 0})
        );
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RequestExecutionMode {
    Local,
    Server,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct HostnameOverride {
    pub hostname: String,
    pub target: String,
}

const MAX_SHARED_HISTORY_BODY_BYTES: usize = 1024 * 1024;
const MAX_SHARED_HISTORY_HEADER_BYTES: usize = 512 * 1024;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ProfileView {
    pub id: String,
    pub email: String,
    pub display_name: String,
    pub active: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SharedHistoryHeader {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SharedHistoryBodyField {
    pub enabled: bool,
    pub name: String,
    pub value: String,
    pub kind: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SharedHistoryRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<SharedHistoryHeader>,
    pub body: String,
    pub body_mode: String,
    pub raw_body_language: String,
    pub body_fields: Vec<SharedHistoryBodyField>,
    pub body_truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SharedHistoryResponse {
    pub status: u16,
    pub status_text: String,
    pub http_version: String,
    pub final_url: String,
    pub headers: Vec<SharedHistoryHeader>,
    pub body_base64: String,
    pub body_truncated: bool,
    #[serde(default)]
    pub content_type: String,
    pub duration_micros: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct SharedHistoryEntry {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub request: SharedHistoryRequest,
    pub response: Option<SharedHistoryResponse>,
    #[serde(default)]
    pub error: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ActivityLogDiff {
    pub field: String,
    pub from: serde_json::Value,
    pub to: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ActivityLogEntry {
    pub id: String,
    pub kind: String,
    pub resource: String,
    pub action: String,
    pub resource_id: String,
    #[serde(default)]
    pub workspace_id: String,
    #[serde(default)]
    pub collection_id: String,
    #[serde(default)]
    pub actor_user_id: String,
    #[serde(default)]
    pub actor_email: String,
    #[serde(default)]
    pub actor_display_name: String,
    pub target_name: String,
    pub diffs: Vec<ActivityLogDiff>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ActivityLogPage {
    pub entries: Vec<ActivityLogEntry>,
    pub older_cursor: Option<String>,
    pub newer_cursor: Option<String>,
    pub has_more_newer: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct SharedHistoryUpload {
    client_entry_id: String,
    created_at: DateTime<Utc>,
    request: SharedHistoryRequest,
    response: Option<SharedHistoryResponse>,
    error: String,
}

impl SharedHistoryUpload {
    pub fn completed(
        client_entry_id: String,
        created_at: DateTime<Utc>,
        request: &RequestDraft,
        response: &ResponseData,
        sensitive_values: &[String],
    ) -> Self {
        let (request, secrets) = request_for_shared_history(request, sensitive_values);
        let response_body_truncated = response.body.len() > MAX_SHARED_HISTORY_BODY_BYTES;
        let mut response = response.clone();
        let redaction_overlap = secrets
            .iter()
            .map(|value| value.len().saturating_mul(3))
            .max()
            .unwrap_or(0)
            .saturating_sub(1);
        let prefix_len = response
            .body
            .len()
            .min(MAX_SHARED_HISTORY_BODY_BYTES.saturating_add(redaction_overlap));
        response.body = response.body.slice(..prefix_len);
        let response = response_for_shared_history(&response, &secrets);
        Self {
            client_entry_id,
            created_at,
            request: shared_request(request),
            response: Some(shared_response(response, response_body_truncated)),
            error: String::new(),
        }
    }

    pub fn failed(
        client_entry_id: String,
        created_at: DateTime<Utc>,
        request: &RequestDraft,
        error: &str,
        sensitive_values: &[String],
    ) -> Self {
        let (request, secrets) = request_for_shared_history(request, sensitive_values);
        Self {
            client_entry_id,
            created_at,
            request: shared_request(request),
            response: None,
            error: truncate_shared_text(
                &super::template::redact_secret_values(error, &secrets),
                65536,
            )
            .0,
        }
    }
}

fn shared_request(mut request: RequestDraft) -> SharedHistoryRequest {
    let (body, mut body_truncated) =
        truncate_shared_text(&request.body, MAX_SHARED_HISTORY_BODY_BYTES);
    let mut remaining = MAX_SHARED_HISTORY_BODY_BYTES;
    let omitted_fields = request.body_fields.len() > 256;
    let mut fields = Vec::new();
    for field in request.body_fields.drain(..).take(256) {
        let name = truncate_shared_text(&field.name, 4096).0;
        let (value, truncated) = if field.kind == BodyFieldKind::File {
            (String::new(), false)
        } else {
            truncate_shared_text(&field.value, remaining)
        };
        remaining = remaining.saturating_sub(value.len());
        body_truncated |= truncated;
        fields.push(SharedHistoryBodyField {
            enabled: field.enabled,
            name,
            value,
            kind: field.kind.as_db_str().to_owned(),
        });
    }
    body_truncated |= omitted_fields;
    SharedHistoryRequest {
        method: truncate_shared_text(&request.method, 64).0,
        url: truncate_shared_text(&request.url, 16384).0,
        headers: shared_headers(
            request
                .headers
                .into_iter()
                .map(|header| (header.name, header.value)),
        ),
        body,
        body_mode: request.body_mode.as_db_str().to_owned(),
        raw_body_language: request.raw_body_language.as_db_str().to_owned(),
        body_fields: fields,
        body_truncated,
    }
}

fn shared_response(response: ResponseData, body_was_truncated: bool) -> SharedHistoryResponse {
    let body_len = response.body.len().min(MAX_SHARED_HISTORY_BODY_BYTES);
    SharedHistoryResponse {
        status: response.status,
        status_text: truncate_shared_text(&response.status_text, 120).0,
        http_version: truncate_shared_text(&response.http_version, 32).0,
        final_url: truncate_shared_text(&response.final_url, 16384).0,
        headers: shared_headers(
            response
                .headers
                .into_iter()
                .map(|header| (header.name, header.value)),
        ),
        body_base64: BASE64_STANDARD.encode(&response.body[..body_len]),
        body_truncated: body_was_truncated || response.body.len() > body_len,
        content_type: truncate_shared_text(response.content_type.as_deref().unwrap_or(""), 512).0,
        duration_micros: response.duration.as_micros().min(i64::MAX as u128) as u64,
    }
}

fn shared_headers(headers: impl IntoIterator<Item = (String, String)>) -> Vec<SharedHistoryHeader> {
    let mut remaining = MAX_SHARED_HISTORY_HEADER_BYTES;
    let mut shared = Vec::new();
    for (name, value) in headers.into_iter().take(256) {
        if remaining == 0 {
            break;
        }
        let name = truncate_shared_text(&name, 4096.min(remaining)).0;
        remaining = remaining.saturating_sub(name.len());
        let value = truncate_shared_text(&value, 65536.min(remaining)).0;
        remaining = remaining.saturating_sub(value.len());
        if !name.is_empty() {
            shared.push(SharedHistoryHeader { name, value });
        }
    }
    shared
}

fn truncate_shared_text(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_owned(), false);
    }
    let mut end = max_bytes.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_owned(), true)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RequestExecutionSettings {
    pub mode: RequestExecutionMode,
    pub hostname_overrides: Vec<HostnameOverride>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ManagementPermission {
    pub key: String,
    pub description: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ManagementRole {
    pub id: String,
    pub name: String,
    pub description: String,
    pub system: bool,
    pub permissions: Vec<ManagementPermission>,
    pub created_by: Option<UpstreamUserSummary>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ManagementUser {
    pub id: String,
    pub email: String,
    pub display_name: String,
    pub active: bool,
    pub roles: Vec<ManagementRole>,
    pub created_by: Option<UpstreamUserSummary>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ManagementUser {
    pub fn has_permission(&self, permission: &str) -> bool {
        self.roles.iter().any(|role| {
            role.permissions
                .iter()
                .any(|assigned| assigned.key == permission)
        })
    }

    #[cfg(test)]
    pub fn permission_keys(&self) -> BTreeSet<&str> {
        self.roles
            .iter()
            .flat_map(|role| role.permissions.iter())
            .map(|permission| permission.key.as_str())
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpstreamManagementSnapshot {
    pub current_user: ManagementUser,
    pub profiles: Vec<ProfileView>,
    pub users: Option<Vec<ManagementUser>>,
    pub roles: Option<Vec<ManagementRole>>,
    pub permissions: Option<Vec<ManagementPermission>>,
    pub workspaces: Option<Vec<UpstreamWorkspaceView>>,
    pub request_execution_settings: Option<RequestExecutionSettings>,
}

impl UpstreamManagementSnapshot {
    pub fn has_permission(&self, permission: &str) -> bool {
        self.current_user.has_permission(permission)
    }
}

#[derive(Debug, Error)]
pub enum UpstreamManagementError {
    #[error("could not reach the server: {0}")]
    Transport(reqwest::Error),
    #[error("the server redirected the request")]
    Redirected,
    #[error("the server returned more than {limit_bytes} bytes")]
    ResponseTooLarge { limit_bytes: usize },
    #[error("the server returned an invalid response: {0}")]
    InvalidResponse(String),
    #[error("{message}")]
    Rejected { status: StatusCode, message: String },
}

#[derive(Serialize)]
struct CreateUserRequest<'a> {
    email: &'a str,
    display_name: &'a str,
    password: &'a str,
    role_ids: &'a [String],
}

#[derive(Serialize)]
struct UpdateUserRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    password: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    active: Option<bool>,
}

#[derive(Serialize)]
struct ReplaceRolesRequest<'a> {
    role_ids: &'a [String],
}

#[derive(Serialize)]
struct CreateRoleRequest<'a> {
    name: &'a str,
    description: &'a str,
    permission_keys: &'a [String],
}

#[derive(Serialize)]
struct UpdateRoleRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
}

#[derive(Serialize)]
struct ReplacePermissionsRequest<'a> {
    permission_keys: &'a [String],
}

#[derive(Serialize)]
struct ReplaceUsersRequest<'a> {
    user_ids: &'a [String],
}

#[derive(Deserialize)]
struct ApiEnvelope<T> {
    success: bool,
    data: Option<T>,
    error: Option<ApiErrorBody>,
}

#[derive(Deserialize)]
struct ApiErrorBody {
    message: String,
    #[serde(default)]
    fields: BTreeMap<String, String>,
}

pub async fn load_upstream_management(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
) -> Result<UpstreamManagementSnapshot, UpstreamManagementError> {
    let current_user: ManagementUser =
        get(client, base_url, bearer_token, "api/v1/auth/me").await?;

    let users_request = async {
        if current_user.has_permission(USERS_READ) {
            get(client, base_url, bearer_token, "api/v1/users")
                .await
                .map(Some)
        } else {
            Ok(None)
        }
    };
    let profiles_request = get(client, base_url, bearer_token, "api/v1/profiles");
    let roles_request = async {
        if current_user.has_permission(ROLES_READ) {
            get(client, base_url, bearer_token, "api/v1/roles")
                .await
                .map(Some)
        } else {
            Ok(None)
        }
    };
    let permissions_request = async {
        if current_user.has_permission(PERMISSIONS_READ) {
            get(client, base_url, bearer_token, "api/v1/permissions")
                .await
                .map(Some)
        } else {
            Ok(None)
        }
    };
    let workspaces_request = async {
        if current_user.has_permission(WORKSPACES_READ)
            && current_user.has_permission(COLLECTIONS_READ)
            && current_user.has_permission(REQUESTS_READ)
        {
            get(client, base_url, bearer_token, "api/v1/workspaces")
                .await
                .map(Some)
        } else {
            Ok(None)
        }
    };
    let request_execution_settings_request = async {
        if current_user.has_permission(SERVER_SETTINGS_READ) {
            get(
                client,
                base_url,
                bearer_token,
                "api/v1/request-execution/settings",
            )
            .await
            .map(Some)
        } else {
            Ok(None)
        }
    };

    let (profiles, users, roles, permissions, workspaces, request_execution_settings) = futures::try_join!(
        profiles_request,
        users_request,
        roles_request,
        permissions_request,
        workspaces_request,
        request_execution_settings_request
    )?;
    Ok(UpstreamManagementSnapshot {
        current_user,
        profiles,
        users,
        roles,
        permissions,
        workspaces,
        request_execution_settings,
    })
}

pub async fn list_workspace_activity(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    cursor: Option<&str>,
    after: Option<&str>,
) -> Result<ActivityLogPage, UpstreamManagementError> {
    list_activity(
        client,
        base_url,
        bearer_token,
        &format!("api/v1/workspaces/{workspace_id}/change-log"),
        cursor,
        after,
    )
    .await
}

pub async fn list_audit_activity(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    cursor: Option<&str>,
    after: Option<&str>,
) -> Result<ActivityLogPage, UpstreamManagementError> {
    list_activity(
        client,
        base_url,
        bearer_token,
        "api/v1/audit-log",
        cursor,
        after,
    )
    .await
}

async fn list_activity(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    path: &str,
    cursor: Option<&str>,
    after: Option<&str>,
) -> Result<ActivityLogPage, UpstreamManagementError> {
    let mut endpoint = endpoint(base_url, path)?;
    {
        let mut query = endpoint.query_pairs_mut();
        query.append_pair("limit", "30");
        if let Some(cursor) = cursor {
            query.append_pair("cursor", cursor);
        }
        if let Some(after) = after {
            query.append_pair("after", after);
        }
    }
    let response = client
        .get(endpoint)
        .bearer_auth(bearer_token)
        .send()
        .await
        .map_err(UpstreamManagementError::Transport)?;
    parse_response(response).await
}

pub async fn list_shared_history(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    user_id: &str,
) -> Result<Vec<SharedHistoryEntry>, UpstreamManagementError> {
    let mut endpoint = endpoint(base_url, &format!("api/v1/profiles/{user_id}/history"))?;
    endpoint
        .query_pairs_mut()
        .append_pair("workspace_id", workspace_id);
    let response = client
        .get(endpoint)
        .bearer_auth(bearer_token)
        .send()
        .await
        .map_err(UpstreamManagementError::Transport)?;
    parse_response(response).await
}

pub async fn upload_shared_history(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    upload: &SharedHistoryUpload,
) -> Result<SharedHistoryEntry, UpstreamManagementError> {
    send(
        client,
        base_url,
        bearer_token,
        Method::POST,
        &format!("api/v1/workspaces/{workspace_id}/history"),
        upload,
    )
    .await
}

pub async fn delete_shared_history(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
) -> Result<(), UpstreamManagementError> {
    let endpoint = endpoint(
        base_url,
        &format!("api/v1/workspaces/{workspace_id}/history"),
    )?;
    let response = client
        .delete(endpoint)
        .bearer_auth(bearer_token)
        .send()
        .await
        .map_err(UpstreamManagementError::Transport)?;
    let _: BTreeMap<String, bool> = parse_response(response).await?;
    Ok(())
}

pub async fn update_request_execution_settings(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    settings: &RequestExecutionSettings,
) -> Result<RequestExecutionSettings, UpstreamManagementError> {
    send(
        client,
        base_url,
        bearer_token,
        Method::PUT,
        "api/v1/request-execution/settings",
        settings,
    )
    .await
}

pub async fn create_management_user(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    email: &str,
    display_name: &str,
    password: &str,
    role_ids: &[String],
) -> Result<ManagementUser, UpstreamManagementError> {
    send(
        client,
        base_url,
        bearer_token,
        Method::POST,
        "api/v1/users",
        &CreateUserRequest {
            email,
            display_name,
            password,
            role_ids,
        },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn update_management_user(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    user_id: &str,
    email: Option<&str>,
    display_name: Option<&str>,
    password: Option<&str>,
    active: Option<bool>,
) -> Result<ManagementUser, UpstreamManagementError> {
    send(
        client,
        base_url,
        bearer_token,
        Method::PATCH,
        &format!("api/v1/users/{user_id}"),
        &UpdateUserRequest {
            email,
            display_name,
            password,
            active,
        },
    )
    .await
}

pub async fn replace_management_user_roles(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    user_id: &str,
    role_ids: &[String],
) -> Result<ManagementUser, UpstreamManagementError> {
    send(
        client,
        base_url,
        bearer_token,
        Method::PUT,
        &format!("api/v1/users/{user_id}/roles"),
        &ReplaceRolesRequest { role_ids },
    )
    .await
}

pub async fn create_management_role(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    name: &str,
    description: &str,
    permission_keys: &[String],
) -> Result<ManagementRole, UpstreamManagementError> {
    send(
        client,
        base_url,
        bearer_token,
        Method::POST,
        "api/v1/roles",
        &CreateRoleRequest {
            name,
            description,
            permission_keys,
        },
    )
    .await
}

pub async fn update_management_role(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    role_id: &str,
    name: Option<&str>,
    description: Option<&str>,
) -> Result<ManagementRole, UpstreamManagementError> {
    send(
        client,
        base_url,
        bearer_token,
        Method::PATCH,
        &format!("api/v1/roles/{role_id}"),
        &UpdateRoleRequest { name, description },
    )
    .await
}

pub async fn replace_management_role_permissions(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    role_id: &str,
    permission_keys: &[String],
) -> Result<ManagementRole, UpstreamManagementError> {
    send(
        client,
        base_url,
        bearer_token,
        Method::PUT,
        &format!("api/v1/roles/{role_id}/permissions"),
        &ReplacePermissionsRequest { permission_keys },
    )
    .await
}

pub async fn replace_management_workspace_users(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    user_ids: &[String],
) -> Result<UpstreamWorkspaceView, UpstreamManagementError> {
    send(
        client,
        base_url,
        bearer_token,
        Method::PUT,
        &format!("api/v1/workspaces/{workspace_id}/users"),
        &ReplaceUsersRequest { user_ids },
    )
    .await
}

pub async fn replace_management_collection_users(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    workspace_id: &str,
    collection_id: &str,
    user_ids: &[String],
) -> Result<UpstreamCollectionView, UpstreamManagementError> {
    send(
        client,
        base_url,
        bearer_token,
        Method::PUT,
        &format!("api/v1/workspaces/{workspace_id}/collections/{collection_id}/users"),
        &ReplaceUsersRequest { user_ids },
    )
    .await
}

async fn get<T: DeserializeOwned>(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    path: &str,
) -> Result<T, UpstreamManagementError> {
    let endpoint = endpoint(base_url, path)?;
    let response = client
        .get(endpoint)
        .bearer_auth(bearer_token)
        .send()
        .await
        .map_err(UpstreamManagementError::Transport)?;
    parse_response(response).await
}

async fn send<T: DeserializeOwned, B: Serialize + ?Sized>(
    client: &Client,
    base_url: &Url,
    bearer_token: &str,
    method: Method,
    path: &str,
    body: &B,
) -> Result<T, UpstreamManagementError> {
    let endpoint = endpoint(base_url, path)?;
    let response = client
        .request(method, endpoint)
        .bearer_auth(bearer_token)
        .json(body)
        .send()
        .await
        .map_err(UpstreamManagementError::Transport)?;
    parse_response(response).await
}

fn endpoint(base_url: &Url, path: &str) -> Result<Url, UpstreamManagementError> {
    base_url
        .join(path)
        .map_err(|error| UpstreamManagementError::InvalidResponse(error.to_string()))
}

async fn parse_response<T: DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, UpstreamManagementError> {
    if response.status().is_redirection() {
        return Err(UpstreamManagementError::Redirected);
    }
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > MANAGEMENT_RESPONSE_LIMIT_BYTES as u64)
    {
        return Err(UpstreamManagementError::ResponseTooLarge {
            limit_bytes: MANAGEMENT_RESPONSE_LIMIT_BYTES,
        });
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(UpstreamManagementError::Transport)?
    {
        if body.len().saturating_add(chunk.len()) > MANAGEMENT_RESPONSE_LIMIT_BYTES {
            return Err(UpstreamManagementError::ResponseTooLarge {
                limit_bytes: MANAGEMENT_RESPONSE_LIMIT_BYTES,
            });
        }
        body.extend_from_slice(&chunk);
    }
    let envelope: ApiEnvelope<T> = serde_json::from_slice(&body)
        .map_err(|error| UpstreamManagementError::InvalidResponse(error.to_string()))?;
    if !status.is_success() || !envelope.success {
        let message = envelope
            .error
            .map(format_api_error)
            .filter(|message| !message.trim().is_empty())
            .unwrap_or_else(|| format!("server request failed with HTTP {status}"));
        return Err(UpstreamManagementError::Rejected { status, message });
    }
    envelope.data.ok_or_else(|| {
        UpstreamManagementError::InvalidResponse(
            "the response did not include the requested data".to_owned(),
        )
    })
}

fn format_api_error(error: ApiErrorBody) -> String {
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    #[test]
    fn effective_permissions_are_deduplicated_across_roles() {
        let permission = ManagementPermission {
            key: USERS_READ.to_owned(),
            description: "View users".to_owned(),
        };
        let role = |id: &str| ManagementRole {
            id: id.to_owned(),
            name: id.to_owned(),
            description: String::new(),
            system: false,
            permissions: vec![permission.clone()],
            created_by: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let user = ManagementUser {
            id: "user".to_owned(),
            email: "owner".to_owned(),
            display_name: "Owner".to_owned(),
            active: true,
            roles: vec![role("one"), role("two")],
            created_by: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        assert!(user.has_permission(USERS_READ));
        assert_eq!(user.permission_keys(), BTreeSet::from([USERS_READ]));
    }

    #[test]
    fn validation_errors_keep_field_context() {
        let message = format_api_error(ApiErrorBody {
            message: "request validation failed".to_owned(),
            fields: BTreeMap::from([
                (
                    "password".to_owned(),
                    "must be at least 12 characters".to_owned(),
                ),
                ("email".to_owned(), "is required".to_owned()),
            ]),
        });

        assert_eq!(
            message,
            "request validation failed (email: is required, password: must be at least 12 characters)"
        );
    }

    #[test]
    fn shared_history_upload_preserves_bodies_and_omits_private_headers() {
        let private_value = "custom-partner-credential";
        let request = RequestDraft {
            method: "POST".to_owned(),
            url: format!("https://example.test/items?echo={private_value}"),
            headers: vec![super::super::HeaderEntry {
                enabled: true,
                shared: false,
                name: "X-Partner-Credential".to_owned(),
                value: private_value.to_owned(),
            }],
            body: format!(r#"{{"echo":"{private_value}"}}"#),
            ..RequestDraft::default()
        };
        let response = ResponseData {
            status: 201,
            status_text: "Created".to_owned(),
            http_version: "HTTP/2".to_owned(),
            final_url: "https://example.test/items/1".to_owned(),
            headers: vec![super::super::request::ResponseHeader {
                name: "Content-Type".to_owned(),
                value: "application/json".to_owned(),
            }],
            content_type: Some("application/json".to_owned()),
            body: br#"{"ok":true}"#.to_vec().into(),
            duration: Duration::from_micros(1250),
        };

        let upload = SharedHistoryUpload::completed(
            "entry-1".to_owned(),
            Utc::now(),
            &request,
            &response,
            &[],
        );
        let json = serde_json::to_value(&upload).unwrap();

        assert_eq!(json["client_entry_id"], "entry-1");
        assert_eq!(json["request"]["headers"], serde_json::json!([]));
        assert!(!json.to_string().contains(private_value));
        assert_eq!(json["response"]["status"], 201);
        assert_eq!(json["response"]["duration_micros"], 1250);
        assert_eq!(
            json["response"]["body_base64"],
            BASE64_STANDARD.encode(br#"{"ok":true}"#)
        );
    }

    #[test]
    fn shared_history_redacts_a_private_header_value_across_the_body_limit() {
        let private_value = "boundary-private-header-value";
        let request = RequestDraft {
            headers: vec![super::super::HeaderEntry {
                enabled: true,
                shared: false,
                name: "X-Partner-Credential".to_owned(),
                value: private_value.to_owned(),
            }],
            ..RequestDraft::default()
        };
        let mut body = vec![b'x'; MAX_SHARED_HISTORY_BODY_BYTES - 8];
        body.extend_from_slice(private_value.as_bytes());
        let response = ResponseData {
            status: 200,
            status_text: "OK".to_owned(),
            http_version: "HTTP/2".to_owned(),
            final_url: "https://example.test".to_owned(),
            headers: Vec::new(),
            content_type: Some("application/octet-stream".to_owned()),
            body: body.into(),
            duration: Duration::from_millis(1),
        };

        let upload = SharedHistoryUpload::completed(
            "entry-boundary".to_owned(),
            Utc::now(),
            &request,
            &response,
            &[],
        );
        let shared = upload.response.expect("completed upload has a response");
        let decoded = BASE64_STANDARD.decode(shared.body_base64).unwrap();

        assert!(shared.body_truncated);
        assert!(decoded.len() <= MAX_SHARED_HISTORY_BODY_BYTES);
        assert!(
            !decoded
                .windows(private_value.len())
                .any(|value| value == private_value.as_bytes())
        );
        let boundary_prefix = &private_value.as_bytes()[..8];
        assert!(
            decoded
                .windows(boundary_prefix.len())
                .all(|value| value != boundary_prefix)
        );
    }
}
