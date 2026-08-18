use std::collections::BTreeMap;

#[cfg(test)]
use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use reqwest::{Client, Method, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;
use url::Url;

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
pub const ENVIRONMENTS_READ: &str = "environments.read";
pub const ENVIRONMENTS_CREATE: &str = "environments.create";
pub const ENVIRONMENTS_UPDATE: &str = "environments.update";
pub const ENVIRONMENTS_DELETE: &str = "environments.delete";
pub const ENVIRONMENT_VALUES_UPDATE: &str = "environment_values.update";

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

    let (users, roles, permissions, workspaces, request_execution_settings) = futures::try_join!(
        users_request,
        roles_request,
        permissions_request,
        workspaces_request,
        request_execution_settings_request
    )?;
    Ok(UpstreamManagementSnapshot {
        current_user,
        users,
        roles,
        permissions,
        workspaces,
        request_execution_settings,
    })
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
}
