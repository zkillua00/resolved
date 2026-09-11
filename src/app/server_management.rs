use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

use gpui::{ListAlignment, ListState};
use gpui_component::group_box::GroupBoxVariant;
use gpui_component::setting::{SettingGroup, SettingItem, SettingPage, Settings as SettingsView};
use gpui_component::switch::Switch;
use zeroize::Zeroizing;

use crate::core::{
    COLLECTIONS_ASSIGN_USERS, HISTORY_READ_OTHERS, ManagementRole, ManagementUser,
    ROLES_ASSIGN_PERMISSIONS, ROLES_CREATE, ROLES_UPDATE, SharedHistoryEntry,
    SharedHistoryResponse, USERS_ASSIGN_ROLES, USERS_CREATE, USERS_UPDATE, UpstreamCollectionView,
    UpstreamManagementSnapshot, UpstreamSavedRequestView, UpstreamUserSummary,
    UpstreamWorkspaceView, WORKSPACES_ASSIGN_USERS, create_management_role, create_management_user,
    list_shared_history, load_upstream_management, replace_management_collection_users,
    replace_management_role_permissions, replace_management_user_roles,
    replace_management_workspace_users, update_management_role, update_management_user,
};

use super::*;

pub(super) mod activity_views;
mod discord_views;
mod execution_limit_views;
mod network_views;
mod profile_views;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum ServerManagementStatus {
    #[default]
    Idle,
    Loading,
    Ready,
    Saving,
    Error(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum ProfileHistoryStatus {
    #[default]
    Idle,
    Loading,
    Ready,
    Error(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum ActivityLogStatus {
    #[default]
    Idle,
    Loading,
    Ready,
    Error(String),
}

#[derive(Clone, Debug, Default)]
pub(super) struct ActivityLogFeed {
    upstream_id: Option<String>,
    workspace_id: Option<String>,
    status: ActivityLogStatus,
    entries: Rc<Vec<crate::core::ActivityLogEntry>>,
    older_cursor: Option<String>,
    newer_cursor: Option<String>,
    loading_more: bool,
    syncing: bool,
    sync_pending: bool,
    error: Option<String>,
    generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RolePermissionDraft {
    baseline: BTreeSet<String>,
    selected: BTreeSet<String>,
}

impl RolePermissionDraft {
    fn new(baseline: BTreeSet<String>) -> Self {
        Self {
            selected: baseline.clone(),
            baseline,
        }
    }

    fn toggle(&mut self, permission_key: &str) {
        if !self.selected.remove(permission_key) {
            self.selected.insert(permission_key.to_owned());
        }
    }

    fn is_dirty(&self) -> bool {
        self.selected != self.baseline
    }

    fn rebase(&mut self, baseline: BTreeSet<String>) {
        let added = self
            .selected
            .difference(&self.baseline)
            .cloned()
            .collect::<Vec<_>>();
        let removed = self
            .baseline
            .difference(&self.selected)
            .cloned()
            .collect::<Vec<_>>();
        self.selected = baseline.clone();
        self.selected.extend(added);
        for permission_key in removed {
            self.selected.remove(&permission_key);
        }
        self.baseline = baseline;
    }
}

impl ServerManagementStatus {
    fn busy(&self) -> bool {
        matches!(self, Self::Loading | Self::Saving)
    }
}

#[derive(Clone, Debug)]
pub(super) struct ServerManagementState {
    pub upstream_id: Option<String>,
    pub status: ServerManagementStatus,
    pub snapshot: Option<UpstreamManagementSnapshot>,
    selected_user_id: Option<String>,
    selected_profile_id: Option<String>,
    selected_profile_history_id: Option<String>,
    profile_history_status: ProfileHistoryStatus,
    profile_history: Vec<SharedHistoryEntry>,
    /// Decoded response-body display strings for profile history entries,
    /// rebuilt by `set_profile_history` so renders never re-decode base64.
    profile_history_body_cache: HashMap<String, String>,
    change_log: ActivityLogFeed,
    audit_log: ActivityLogFeed,
    change_log_list: ListState,
    audit_log_list: ListState,
    selected_role_id: Option<String>,
    role_permission_drafts: BTreeMap<String, RolePermissionDraft>,
    selected_resource: Option<ManagementResourceSelection>,
    realtime_refresh_pending: bool,
    execution_limits: execution_limit_views::ExecutionLimitState,
}

impl Default for ServerManagementState {
    fn default() -> Self {
        Self {
            upstream_id: None,
            status: ServerManagementStatus::default(),
            snapshot: None,
            selected_user_id: None,
            selected_profile_id: None,
            selected_profile_history_id: None,
            profile_history_status: ProfileHistoryStatus::Idle,
            profile_history: Vec::new(),
            profile_history_body_cache: HashMap::new(),
            change_log: ActivityLogFeed::default(),
            audit_log: ActivityLogFeed::default(),
            change_log_list: ListState::new(0, ListAlignment::Top, px(200.)),
            audit_log_list: ListState::new(0, ListAlignment::Top, px(200.)),
            selected_role_id: None,
            role_permission_drafts: BTreeMap::new(),
            selected_resource: None,
            realtime_refresh_pending: false,
            execution_limits: execution_limit_views::ExecutionLimitState::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ManagementResourceSelection {
    Workspace(String),
    Collection(String),
    Request(String),
}

impl ServerManagementState {
    fn toggle_role_permission(&mut self, role_id: &str, permission_key: &str) {
        let Some(baseline) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.roles.as_ref())
            .and_then(|roles| roles.iter().find(|role| role.id == role_id))
            .filter(|role| !role.system)
            .map(management_role_permission_keys)
        else {
            return;
        };
        let draft = self
            .role_permission_drafts
            .entry(role_id.to_owned())
            .or_insert_with(|| RolePermissionDraft::new(baseline));
        draft.toggle(permission_key);
        if !draft.is_dirty() {
            self.role_permission_drafts.remove(role_id);
        }
    }

    fn reset_role_permission_draft(&mut self, role_id: &str) {
        self.role_permission_drafts.remove(role_id);
    }

    fn reconcile_role_permission_drafts(&mut self, snapshot: &UpstreamManagementSnapshot) {
        if !snapshot.has_permission(ROLES_ASSIGN_PERMISSIONS) {
            self.role_permission_drafts.clear();
            return;
        }
        let Some(roles) = snapshot.roles.as_ref() else {
            self.role_permission_drafts.clear();
            return;
        };
        self.role_permission_drafts.retain(|role_id, draft| {
            let Some(role) = roles
                .iter()
                .find(|role| &role.id == role_id)
                .filter(|role| !role.system)
            else {
                return false;
            };
            draft.rebase(management_role_permission_keys(role));
            draft.is_dirty()
        });
    }

    pub(super) fn reset_profile_history(&mut self) {
        self.profile_history_status = ProfileHistoryStatus::Idle;
        self.profile_history.clear();
        self.profile_history_body_cache.clear();
        self.selected_profile_history_id = None;
    }

    fn set_profile_history(&mut self, entries: Vec<SharedHistoryEntry>) {
        self.profile_history_body_cache = entries
            .iter()
            .filter_map(|entry| {
                entry
                    .response
                    .as_ref()
                    .map(|response| (entry.id.clone(), shared_history_response_body(response)))
            })
            .collect();
        self.profile_history = entries;
    }

    pub(super) fn is_history_visible_for(&self, user_id: &str) -> bool {
        if matches!(self.profile_history_status, ProfileHistoryStatus::Idle)
            || self.selected_profile_id.as_deref() != Some(user_id)
        {
            return false;
        }
        self.snapshot.as_ref().is_some_and(|snapshot| {
            user_id == snapshot.current_user.id || snapshot.has_permission(HISTORY_READ_OTHERS)
        })
    }

    fn set_snapshot(&mut self, snapshot: UpstreamManagementSnapshot) {
        self.reconcile_role_permission_drafts(&snapshot);
        if !snapshot.has_permission(crate::core::AUDIT_READ) {
            self.audit_log = ActivityLogFeed::default();
        }
        if !snapshot.has_permission(crate::core::WORKSPACES_READ)
            || !snapshot.has_permission(crate::core::COLLECTIONS_READ)
            || !snapshot.has_permission(crate::core::REQUESTS_READ)
        {
            self.change_log = ActivityLogFeed::default();
        }
        if !snapshot
            .profiles
            .iter()
            .any(|profile| self.selected_profile_id.as_ref() == Some(&profile.id))
        {
            self.selected_profile_id = snapshot
                .profiles
                .iter()
                .find(|profile| profile.id == snapshot.current_user.id)
                .or_else(|| snapshot.profiles.first())
                .map(|profile| profile.id.clone());
            self.reset_profile_history();
        }
        if self.selected_profile_id.as_deref() != Some(snapshot.current_user.id.as_str())
            && !snapshot.has_permission(HISTORY_READ_OTHERS)
        {
            self.reset_profile_history();
        }
        if !snapshot.users.as_ref().is_some_and(|users| {
            self.selected_user_id
                .as_ref()
                .is_some_and(|selected| users.iter().any(|user| &user.id == selected))
        }) {
            self.selected_user_id = snapshot
                .users
                .as_ref()
                .and_then(|users| users.first())
                .map(|user| user.id.clone());
        }
        if !snapshot.roles.as_ref().is_some_and(|roles| {
            self.selected_role_id
                .as_ref()
                .is_some_and(|selected| roles.iter().any(|role| &role.id == selected))
        }) {
            self.selected_role_id = snapshot
                .roles
                .as_ref()
                .and_then(|roles| roles.first())
                .map(|role| role.id.clone());
        }
        if !snapshot.workspaces.as_ref().is_some_and(|workspaces| {
            self.selected_resource.as_ref().is_some_and(|selected| {
                discord_views::resource_selection_exists(workspaces, selected)
            })
        }) {
            self.selected_resource = snapshot
                .workspaces
                .as_ref()
                .and_then(|workspaces| workspaces.first())
                .map(|workspace| ManagementResourceSelection::Workspace(workspace.id.clone()));
        }
        self.snapshot = Some(snapshot);
    }
}

fn management_role_permission_keys(role: &ManagementRole) -> BTreeSet<String> {
    role.permissions
        .iter()
        .map(|permission| permission.key.clone())
        .collect()
}

fn shared_history_response_body(response: &SharedHistoryResponse) -> String {
    let Ok(body) = BASE64_STANDARD.decode(&response.body_base64) else {
        return "Invalid shared response body".to_owned();
    };
    match String::from_utf8(body) {
        Ok(body) => body,
        Err(error) => format!("Binary response body ({} bytes)", error.into_bytes().len()),
    }
}

enum ManagementMutation {
    CreateUser {
        login: String,
        display_name: String,
        password: Zeroizing<String>,
    },
    UpdateUser {
        user_id: String,
        login: Option<String>,
        display_name: Option<String>,
        password: Option<Zeroizing<String>>,
        active: Option<bool>,
    },
    ReplaceUserRoles {
        user_id: String,
        role_ids: Vec<String>,
    },
    CreateRole {
        name: String,
        description: String,
    },
    UpdateRole {
        role_id: String,
        name: String,
        description: String,
    },
    ReplaceRolePermissions {
        role_id: String,
        permission_keys: Vec<String>,
    },
    ReplaceWorkspaceUsers {
        workspace_id: String,
        user_ids: Vec<String>,
    },
    ReplaceCollectionUsers {
        workspace_id: String,
        collection_id: String,
        user_ids: Vec<String>,
    },
    UpdateRequestExecutionSettings {
        settings: RequestExecutionSettings,
    },
}

impl ManagementMutation {
    async fn execute(
        self,
        client: &Client,
        base_url: &url::Url,
        bearer_token: &str,
    ) -> Result<String, String> {
        match self {
            Self::CreateUser {
                login,
                display_name,
                password,
            } => {
                create_management_user(
                    client,
                    base_url,
                    bearer_token,
                    &login,
                    &display_name,
                    password.as_str(),
                    &[],
                )
                .await
                .map_err(|error| error.to_string())?;
                Ok(format!("Created {display_name}."))
            }
            Self::UpdateUser {
                user_id,
                login,
                display_name,
                password,
                active,
            } => {
                let user = update_management_user(
                    client,
                    base_url,
                    bearer_token,
                    &user_id,
                    login.as_deref(),
                    display_name.as_deref(),
                    password.as_deref().map(|password| password.as_str()),
                    active,
                )
                .await
                .map_err(|error| error.to_string())?;
                Ok(format!("Updated {}.", user.display_name))
            }
            Self::ReplaceUserRoles { user_id, role_ids } => {
                let user = replace_management_user_roles(
                    client,
                    base_url,
                    bearer_token,
                    &user_id,
                    &role_ids,
                )
                .await
                .map_err(|error| error.to_string())?;
                Ok(format!("Updated roles for {}.", user.display_name))
            }
            Self::CreateRole { name, description } => {
                create_management_role(client, base_url, bearer_token, &name, &description, &[])
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(format!("Created {name}."))
            }
            Self::UpdateRole {
                role_id,
                name,
                description,
            } => {
                let role = update_management_role(
                    client,
                    base_url,
                    bearer_token,
                    &role_id,
                    Some(&name),
                    Some(&description),
                )
                .await
                .map_err(|error| error.to_string())?;
                Ok(format!("Updated {}.", role.name))
            }
            Self::ReplaceRolePermissions {
                role_id,
                permission_keys,
            } => {
                let role = replace_management_role_permissions(
                    client,
                    base_url,
                    bearer_token,
                    &role_id,
                    &permission_keys,
                )
                .await
                .map_err(|error| error.to_string())?;
                Ok(format!("Updated permissions for {}.", role.name))
            }
            Self::ReplaceWorkspaceUsers {
                workspace_id,
                user_ids,
            } => {
                let workspace = replace_management_workspace_users(
                    client,
                    base_url,
                    bearer_token,
                    &workspace_id,
                    &user_ids,
                )
                .await
                .map_err(|error| error.to_string())?;
                Ok(format!("Updated access to {}.", workspace.name))
            }
            Self::ReplaceCollectionUsers {
                workspace_id,
                collection_id,
                user_ids,
            } => {
                let collection = replace_management_collection_users(
                    client,
                    base_url,
                    bearer_token,
                    &workspace_id,
                    &collection_id,
                    &user_ids,
                )
                .await
                .map_err(|error| error.to_string())?;
                Ok(format!("Updated access to {}.", collection.name))
            }
            Self::UpdateRequestExecutionSettings { settings } => {
                let updated =
                    update_request_execution_settings(client, base_url, bearer_token, &settings)
                        .await
                        .map_err(|error| error.to_string())?;
                Ok(match updated.mode {
                    RequestExecutionMode::Local => {
                        "Requests from server workspaces will run on each user's computer."
                            .to_owned()
                    }
                    RequestExecutionMode::Server => {
                        "Requests from server workspaces will run from this server.".to_owned()
                    }
                })
            }
        }
    }
}

impl ApiTester {
    fn sync_upstream_profile_permissions(
        &mut self,
        upstream_id: &str,
        user: &ManagementUser,
        cx: &mut Context<Self>,
    ) {
        let permission_keys = user
            .roles
            .iter()
            .flat_map(|role| role.permissions.iter())
            .map(|permission| permission.key.clone())
            .collect::<BTreeSet<_>>();
        let mut settings = self.settings.clone();
        let Some(profile) = settings
            .upstreams
            .servers
            .iter_mut()
            .find(|profile| profile.id == upstream_id)
        else {
            return;
        };
        if profile.permission_keys == permission_keys {
            return;
        }
        profile.replace_permissions(permission_keys);
        if let Err(error) = self.commit_settings(settings, false, cx) {
            self.settings_notice = Some(error);
        }
    }

    pub(super) fn ensure_server_management_loaded(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active_id = self.settings.upstreams.active_upstream_id.as_deref();
        let current = self.server_management.upstream_id.as_deref();
        if active_id != current
            || matches!(
                self.server_management.status,
                ServerManagementStatus::Idle | ServerManagementStatus::Error(_)
            )
        {
            self.refresh_server_management(window, cx);
        }
    }

    pub(super) fn refresh_server_management_realtime(
        &mut self,
        upstream_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.upstreams.active_upstream_id.as_deref() != Some(upstream_id) {
            return;
        }
        if self.server_management.status.busy() {
            self.server_management.realtime_refresh_pending = true;
            return;
        }
        self.server_management.realtime_refresh_pending = false;
        if matches!(
            self.workspace_tabs.active(),
            ActiveWorkspaceTab::RequestProxy | ActiveWorkspaceTab::ServerTools
        ) {
            self.refresh_server_management(window, cx);
        } else if self.server_management.upstream_id.as_deref() == Some(upstream_id) {
            // Keep the existing snapshot available to non-management surfaces,
            // but force a fresh load before it is rendered again.
            self.server_management.status = ServerManagementStatus::Idle;
            cx.notify();
        }
    }

    pub(super) fn refresh_server_management(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(abort_handle) = self.server_management_abort_handle.take() {
            abort_handle.abort();
        }
        self.server_management_generation = self.server_management_generation.wrapping_add(1);
        let generation = self.server_management_generation;

        let Some(profile) = self.settings.upstreams.active().cloned() else {
            self.server_management = ServerManagementState::default();
            cx.notify();
            return;
        };
        let upstream_id = profile.id.clone();
        if profile.session_expired(Utc::now()) {
            self.server_management = ServerManagementState {
                upstream_id: Some(upstream_id),
                status: ServerManagementStatus::Error(
                    "Log in to this server again to manage it.".to_owned(),
                ),
                snapshot: None,
                ..ServerManagementState::default()
            };
            cx.notify();
            return;
        }
        let Some(base_url) = profile.parsed_base_url() else {
            self.server_management = ServerManagementState {
                upstream_id: Some(upstream_id),
                status: ServerManagementStatus::Error("The server URL is invalid.".to_owned()),
                snapshot: None,
                ..ServerManagementState::default()
            };
            cx.notify();
            return;
        };
        let change_log = if self.server_management.upstream_id.as_deref() == Some(&upstream_id) {
            self.server_management.change_log.clone()
        } else {
            ActivityLogFeed::default()
        };
        let audit_log = if self.server_management.upstream_id.as_deref() == Some(&upstream_id) {
            self.server_management.audit_log.clone()
        } else {
            ActivityLogFeed::default()
        };
        let role_permission_drafts =
            if self.server_management.upstream_id.as_deref() == Some(&upstream_id) {
                self.server_management.role_permission_drafts.clone()
            } else {
                BTreeMap::new()
            };
        let selected_role_id = (self.server_management.upstream_id.as_deref()
            == Some(&upstream_id))
        .then(|| self.server_management.selected_role_id.clone())
        .flatten();
        let execution_limits =
            if self.server_management.upstream_id.as_deref() == Some(&upstream_id) {
                self.server_management.execution_limits.clone()
            } else {
                execution_limit_views::ExecutionLimitState::default()
            };
        self.server_management = ServerManagementState {
            upstream_id: Some(upstream_id.clone()),
            status: ServerManagementStatus::Loading,
            snapshot: None,
            change_log,
            audit_log,
            selected_role_id,
            role_permission_drafts,
            execution_limits,
            ..ServerManagementState::default()
        };
        let vault = self.credential_vault.clone();
        let runtime = Arc::clone(&self.runtime);
        let client = self.upstream_client.clone();
        let task_upstream_id = upstream_id.clone();
        let task = self.runtime.spawn(async move {
            let credential = runtime
                .spawn_blocking(move || vault.load_upstream(&task_upstream_id))
                .await
                .map_err(|error| format!("Could not open the saved session: {error}"))?
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Log in to this server again.".to_owned())?;
            load_upstream_management(&client, &base_url, credential.bearer_token())
                .await
                .map_err(|error| error.to_string())
        });
        self.server_management_abort_handle = Some(task.abort_handle());
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.server_management_generation != generation
                    || this.server_management.upstream_id.as_deref() != Some(upstream_id.as_str())
                {
                    return;
                }
                this.server_management_abort_handle = None;
                match result {
                    Ok(Ok(snapshot)) => {
                        if let Some(abort_handle) = this.profile_history_abort_handle.take() {
                            abort_handle.abort();
                        }
                        this.profile_history_generation =
                            this.profile_history_generation.wrapping_add(1);
                        this.server_management.reset_profile_history();
                        this.sync_upstream_profile_permissions(
                            &upstream_id,
                            &snapshot.current_user,
                            cx,
                        );
                        this.server_management.status = ServerManagementStatus::Ready;
                        this.server_management.set_snapshot(snapshot);
                    }
                    Ok(Err(error)) => {
                        this.server_management.status = ServerManagementStatus::Error(error);
                        this.server_management.snapshot = None;
                    }
                    Err(error) if error.is_cancelled() => return,
                    Err(error) => {
                        this.server_management.status = ServerManagementStatus::Error(format!(
                            "The server settings could not be loaded: {error}"
                        ));
                        this.server_management.snapshot = None;
                    }
                }
                let realtime_refresh_pending =
                    std::mem::take(&mut this.server_management.realtime_refresh_pending);
                cx.notify();
                if realtime_refresh_pending {
                    this.refresh_server_management_realtime(&upstream_id, window, cx);
                }
            });
        })
        .detach();
    }

    fn run_management_mutation(
        &mut self,
        mutation: ManagementMutation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.server_management.status.busy() {
            return;
        }
        let Some(profile) = self.settings.upstreams.active().cloned() else {
            self.settings_notice = Some("Select a server first.".to_owned());
            cx.notify();
            return;
        };
        let Some(base_url) = profile.parsed_base_url() else {
            self.settings_notice = Some("The server URL is invalid.".to_owned());
            cx.notify();
            return;
        };
        if let Some(abort_handle) = self.server_management_abort_handle.take() {
            abort_handle.abort();
        }
        self.server_management_generation = self.server_management_generation.wrapping_add(1);
        let generation = self.server_management_generation;
        let upstream_id = profile.id.clone();
        self.server_management.status = ServerManagementStatus::Saving;
        let vault = self.credential_vault.clone();
        let runtime = Arc::clone(&self.runtime);
        let client = self.upstream_client.clone();
        let task_upstream_id = upstream_id.clone();
        let task = self.runtime.spawn(async move {
            let credential = runtime
                .spawn_blocking(move || vault.load_upstream(&task_upstream_id))
                .await
                .map_err(|error| format!("Could not open the saved session: {error}"))?
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Log in to this server again.".to_owned())?;
            let notice = mutation
                .execute(&client, &base_url, credential.bearer_token())
                .await?;
            let snapshot = load_upstream_management(&client, &base_url, credential.bearer_token())
                .await
                .map_err(|error| error.to_string())?;
            Ok::<_, String>((notice, snapshot))
        });
        self.server_management_abort_handle = Some(task.abort_handle());
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.server_management_generation != generation
                    || this.server_management.upstream_id.as_deref() != Some(upstream_id.as_str())
                {
                    return;
                }
                this.server_management_abort_handle = None;
                match result {
                    Ok(Ok((notice, snapshot))) => {
                        if let Some(abort_handle) = this.profile_history_abort_handle.take() {
                            abort_handle.abort();
                        }
                        this.profile_history_generation =
                            this.profile_history_generation.wrapping_add(1);
                        this.server_management.reset_profile_history();
                        this.sync_upstream_profile_permissions(
                            &upstream_id,
                            &snapshot.current_user,
                            cx,
                        );
                        this.server_management.status = ServerManagementStatus::Ready;
                        this.server_management.set_snapshot(snapshot);
                        this.settings_notice = Some(notice);
                    }
                    Ok(Err(error)) => {
                        this.server_management.status = ServerManagementStatus::Ready;
                        this.settings_notice = Some(error);
                    }
                    Err(error) if error.is_cancelled() => return,
                    Err(error) => {
                        this.server_management.status = ServerManagementStatus::Ready;
                        this.settings_notice =
                            Some(format!("The server change could not be saved: {error}"));
                    }
                }
                let realtime_refresh_pending =
                    std::mem::take(&mut this.server_management.realtime_refresh_pending);
                cx.notify();
                if realtime_refresh_pending {
                    this.refresh_server_management_realtime(&upstream_id, window, cx);
                }
            });
        })
        .detach();
    }

    fn save_role_permission_draft(
        &mut self,
        role_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(draft) = self.server_management.role_permission_drafts.get(role_id) else {
            return;
        };
        self.run_management_mutation(
            ManagementMutation::ReplaceRolePermissions {
                role_id: role_id.to_owned(),
                permission_keys: draft.selected.iter().cloned().collect(),
            },
            window,
            cx,
        );
    }

    pub(super) fn render_server_tools_title_bar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .h(APP_TITLE_BAR_HEIGHT)
            .flex_shrink_0()
            .pl(window_chrome::leading_inset())
            .pr(window_chrome::trailing_inset())
            .border_b_1()
            .border_color(cx.theme().title_bar_border)
            .bg(cx.theme().title_bar)
            .child(
                h_flex().gap_6().child(resolved_brand_lockup(cx)).child(
                    h_flex()
                        .h_full()
                        .items_center()
                        .border_b_2()
                        .border_color(cx.theme().primary)
                        .px_1()
                        .text_sm()
                        .font_semibold()
                        .child("Server Tools"),
                ),
            )
            .child(window_chrome::caption_drag_region())
            .child(window_chrome::window_controls(window, cx))
            .into_any_element()
    }

    pub(super) fn render_server_tools_workspace(&self, cx: &mut Context<Self>) -> AnyElement {
        if !matches!(
            self.workspace_providers.active_id(),
            WorkspaceProviderId::Upstream { .. }
        ) {
            return management_empty("Open a server workspace to use Server Tools.", cx);
        }
        let pages = [
            self.profile_settings_page(cx),
            self.user_management_settings_page(cx),
            self.role_management_settings_page(cx),
            self.resource_management_settings_page(cx),
            self.change_log_settings_page(cx),
            self.audit_log_settings_page(cx),
        ];
        let (message_inset, message_overlay) = super::settings_page::settings_message_overlay(
            self.settings_warning.clone(),
            self.settings_notice.clone(),
            cx,
        );

        v_flex()
            .debug_selector(|| "server-tools-workspace".to_owned())
            .relative()
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            .child(
                div().flex_1().min_h_0().child(
                    SettingsView::new("api-tester-server-tools")
                        .sidebar_width(
                            super::settings_page::SETTINGS_SIDEBAR_WIDTH
                                .to_pixels(cx.theme().font_size),
                        )
                        .content_top_inset(message_inset)
                        .with_group_variant(GroupBoxVariant::Outline)
                        .pages(pages),
                ),
            )
            .when_some(message_overlay, |this, overlay| this.child(overlay))
            .into_any_element()
    }

    pub(super) fn user_management_settings_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let this = cx.entity().downgrade();
        SettingPage::new("Users")
            .description("Create accounts, update profiles, and assign roles.")
            .resettable(false)
            .full_bleed()
            .group(SettingGroup::new().item(SettingItem::render_searchable(
                "users accounts login roles active inactive",
                move |_, _, cx| render_user_management(&this, cx),
            )))
    }

    pub(super) fn role_management_settings_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let this = cx.entity().downgrade();
        SettingPage::new("Roles")
            .description("Group permissions into roles that can be assigned to users.")
            .resettable(false)
            .full_bleed()
            .group(SettingGroup::new().item(SettingItem::render_searchable(
                "roles permissions capabilities access",
                move |_, _, cx| render_role_management(&this, cx),
            )))
    }

    pub(super) fn profile_settings_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let this = cx.entity().downgrade();
        SettingPage::new("Profiles")
            .description("Open a server member's profile and authorized shared request history.")
            .resettable(false)
            .full_bleed()
            .group(SettingGroup::new().item(SettingItem::render_searchable(
                "profiles members shared request history headers bodies",
                move |_, _, cx| profile_views::render_profiles(&this, cx),
            )))
    }

    pub(super) fn change_log_settings_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let this = cx.entity().downgrade();
        SettingPage::new("Change log")
            .description("Inspect field-level request, collection, and workspace changes.")
            .resettable(false)
            .full_bleed()
            .group(SettingGroup::new().item(SettingItem::render_searchable(
                "change log diffs requests collections workspaces from to",
                move |_, window, cx| activity_views::render_change_log(&this, window, cx),
            )))
    }

    pub(super) fn audit_log_settings_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let this = cx.entity().downgrade();
        SettingPage::new("Audit log")
            .description("Review permissioned user and role changes with before/after values.")
            .resettable(false)
            .full_bleed()
            .group(SettingGroup::new().item(SettingItem::render_searchable(
                "audit log users roles permissions before after",
                move |_, window, cx| activity_views::render_audit_log(&this, window, cx),
            )))
    }

    pub(super) fn load_profile_history(
        &mut self,
        user_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.load_profile_history_selecting(user_id, None, window, cx);
    }

    pub(super) fn refresh_profile_history_realtime(
        &mut self,
        user_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected_entry_id = self.server_management.selected_profile_history_id.clone();
        self.load_profile_history_selecting(user_id, selected_entry_id, window, cx);
    }

    fn load_profile_history_selecting(
        &mut self,
        user_id: String,
        preferred_entry_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self.server_management.snapshot.as_ref() else {
            return;
        };
        if user_id != snapshot.current_user.id && !snapshot.has_permission(HISTORY_READ_OTHERS) {
            if let Some(abort_handle) = self.profile_history_abort_handle.take() {
                abort_handle.abort();
            }
            self.profile_history_generation = self.profile_history_generation.wrapping_add(1);
            self.server_management.selected_profile_id = Some(user_id);
            self.server_management.profile_history_status = ProfileHistoryStatus::Error(
                "You do not have permission to view this member's history.".to_owned(),
            );
            self.server_management.profile_history.clear();
            self.server_management.selected_profile_history_id = None;
            cx.notify();
            return;
        }
        let Ok(target) = self.active_upstream_workspace() else {
            self.server_management.profile_history_status = ProfileHistoryStatus::Error(
                "Select an accessible server workspace to view shared history.".to_owned(),
            );
            cx.notify();
            return;
        };
        if let Some(abort_handle) = self.profile_history_abort_handle.take() {
            abort_handle.abort();
        }
        self.profile_history_generation = self.profile_history_generation.wrapping_add(1);
        let generation = self.profile_history_generation;
        self.server_management.selected_profile_id = Some(user_id.clone());
        self.server_management.profile_history_status = ProfileHistoryStatus::Loading;
        self.server_management.profile_history.clear();
        self.server_management.selected_profile_history_id = None;

        let vault = self.credential_vault.clone();
        let client = self.upstream_client.clone();
        let runtime = Arc::clone(&self.runtime);
        let upstream_id = target.upstream_id.clone();
        let selected_user_id = user_id.clone();
        let task = self.runtime.spawn(async move {
            let credential = runtime
                .spawn_blocking(move || vault.load_upstream(&upstream_id))
                .await
                .map_err(|error| format!("Could not open the saved session: {error}"))?
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Log in to this server again.".to_owned())?;
            if credential.expires_at <= Utc::now() {
                return Err("Log in to this server again.".to_owned());
            }
            list_shared_history(
                &client,
                &target.base_url,
                credential.bearer_token(),
                &target.workspace_id,
                &selected_user_id,
            )
            .await
            .map_err(|error| error.to_string())
        });
        self.profile_history_abort_handle = Some(task.abort_handle());
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, _, cx| {
                if this.profile_history_generation != generation
                    || this.server_management.selected_profile_id.as_deref()
                        != Some(user_id.as_str())
                {
                    return;
                }
                this.profile_history_abort_handle = None;
                match result {
                    Ok(Ok(entries)) => {
                        this.server_management.selected_profile_history_id = preferred_entry_id
                            .filter(|selected| entries.iter().any(|entry| &entry.id == selected))
                            .or_else(|| entries.first().map(|entry| entry.id.clone()));
                        this.server_management.set_profile_history(entries);
                        this.server_management.profile_history_status = ProfileHistoryStatus::Ready;
                    }
                    Ok(Err(error)) => {
                        this.server_management.profile_history_status =
                            ProfileHistoryStatus::Error(error);
                    }
                    Err(error) if error.is_cancelled() => return,
                    Err(error) => {
                        this.server_management.profile_history_status = ProfileHistoryStatus::Error(
                            format!("Shared history could not be loaded: {error}"),
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn resource_management_settings_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let this = cx.entity().downgrade();
        SettingPage::new("Resources")
            .description("Manage direct access to workspaces and collection trees.")
            .resettable(false)
            .full_bleed()
            .group(SettingGroup::new().item(SettingItem::render_searchable(
                "resources workspaces collections requests access users creator",
                move |_, _, cx| render_resource_management(&this, cx),
            )))
    }

    fn open_create_user_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let login = cx.new(|cx| InputState::new(window, cx).placeholder("Login"));
        let display_name = cx.new(|cx| InputState::new(window, cx).placeholder("Display name"));
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Password")
                .masked(true)
        });
        let this = cx.entity().downgrade();
        let dialog_login = login.clone();
        let dialog_display_name = display_name.clone();
        let dialog_password = password.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let create_this = this.clone();
            let create_login = dialog_login.clone();
            let create_display_name = dialog_display_name.clone();
            let create_password = dialog_password.clone();
            dialog
                .title("New user")
                .w(px(480.))
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("Create user"))
                .on_ok(move |_, window, cx| {
                    let login = create_login.read(cx).value().trim().to_owned();
                    let display_name = create_display_name.read(cx).value().trim().to_owned();
                    let password = Zeroizing::new(create_password.read(cx).value().to_string());
                    if login.is_empty() || display_name.is_empty() || password.is_empty() {
                        return false;
                    }
                    create_password.update(cx, |input, cx| input.set_value("", window, cx));
                    if let Some(this) = create_this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.run_management_mutation(
                                ManagementMutation::CreateUser {
                                    login: login.clone(),
                                    display_name: display_name.clone(),
                                    password,
                                },
                                window,
                                cx,
                            );
                        });
                    }
                    true
                })
                .child(
                    v_flex()
                        .gap_4()
                        .child(management_dialog_field("LOGIN", Input::new(&dialog_login)))
                        .child(management_dialog_field(
                            "DISPLAY NAME",
                            Input::new(&dialog_display_name),
                        ))
                        .child(management_dialog_field(
                            "PASSWORD",
                            Input::new(&dialog_password).mask_toggle(),
                        )),
                )
        });
        login.read(cx).focus_handle(cx).focus(window);
    }

    fn open_edit_user_dialog(
        &mut self,
        user: ManagementUser,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let login = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Login")
                .default_value(user.email.clone())
        });
        let display_name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Display name")
                .default_value(user.display_name.clone())
        });
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Leave blank to keep the current password")
                .masked(true)
        });
        let user_id = user.id.clone();
        let title = format!("Edit {}", user.display_name);
        let this = cx.entity().downgrade();
        let dialog_login = login.clone();
        let dialog_display_name = display_name.clone();
        let dialog_password = password.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let edit_this = this.clone();
            let edit_user_id = user_id.clone();
            let edit_login = dialog_login.clone();
            let edit_display_name = dialog_display_name.clone();
            let edit_password = dialog_password.clone();
            dialog
                .title(title.clone())
                .w(px(480.))
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("Save user"))
                .on_ok(move |_, window, cx| {
                    let login = edit_login.read(cx).value().trim().to_owned();
                    let display_name = edit_display_name.read(cx).value().trim().to_owned();
                    let password = Zeroizing::new(edit_password.read(cx).value().to_string());
                    if login.is_empty() || display_name.is_empty() {
                        return false;
                    }
                    edit_password.update(cx, |input, cx| input.set_value("", window, cx));
                    if let Some(this) = edit_this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.run_management_mutation(
                                ManagementMutation::UpdateUser {
                                    user_id: edit_user_id.clone(),
                                    login: Some(login.clone()),
                                    display_name: Some(display_name.clone()),
                                    password: if password.is_empty() {
                                        None
                                    } else {
                                        Some(password)
                                    },
                                    active: None,
                                },
                                window,
                                cx,
                            );
                        });
                    }
                    true
                })
                .child(
                    v_flex()
                        .gap_4()
                        .child(management_dialog_field("LOGIN", Input::new(&dialog_login)))
                        .child(management_dialog_field(
                            "DISPLAY NAME",
                            Input::new(&dialog_display_name),
                        ))
                        .child(management_dialog_field(
                            "NEW PASSWORD",
                            Input::new(&dialog_password).mask_toggle(),
                        )),
                )
        });
        login.read(cx).focus_handle(cx).focus(window);
    }

    fn open_create_role_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Role name"));
        let description = cx.new(|cx| InputState::new(window, cx).placeholder("Description"));
        let this = cx.entity().downgrade();
        let dialog_name = name.clone();
        let dialog_description = description.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let create_this = this.clone();
            let create_name = dialog_name.clone();
            let create_description = dialog_description.clone();
            dialog
                .title("New role")
                .w(px(480.))
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("Create role"))
                .on_ok(move |_, window, cx| {
                    let name = create_name.read(cx).value().trim().to_owned();
                    let description = create_description.read(cx).value().trim().to_owned();
                    if name.is_empty() {
                        return false;
                    }
                    if let Some(this) = create_this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.run_management_mutation(
                                ManagementMutation::CreateRole {
                                    name: name.clone(),
                                    description: description.clone(),
                                },
                                window,
                                cx,
                            );
                        });
                    }
                    true
                })
                .child(
                    v_flex()
                        .gap_4()
                        .child(management_dialog_field("NAME", Input::new(&dialog_name)))
                        .child(management_dialog_field(
                            "DESCRIPTION",
                            Input::new(&dialog_description),
                        )),
                )
        });
        name.read(cx).focus_handle(cx).focus(window);
    }

    fn open_edit_role_dialog(
        &mut self,
        role: ManagementRole,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Role name")
                .default_value(role.name.clone())
        });
        let description = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Description")
                .default_value(role.description.clone())
        });
        let role_id = role.id.clone();
        let title = format!("Edit {}", role.name);
        let this = cx.entity().downgrade();
        let dialog_name = name.clone();
        let dialog_description = description.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let edit_this = this.clone();
            let edit_role_id = role_id.clone();
            let edit_name = dialog_name.clone();
            let edit_description = dialog_description.clone();
            dialog
                .title(title.clone())
                .w(px(480.))
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("Save role"))
                .on_ok(move |_, window, cx| {
                    let name = edit_name.read(cx).value().trim().to_owned();
                    let description = edit_description.read(cx).value().trim().to_owned();
                    if name.is_empty() {
                        return false;
                    }
                    if let Some(this) = edit_this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.run_management_mutation(
                                ManagementMutation::UpdateRole {
                                    role_id: edit_role_id.clone(),
                                    name: name.clone(),
                                    description: description.clone(),
                                },
                                window,
                                cx,
                            );
                        });
                    }
                    true
                })
                .child(
                    v_flex()
                        .gap_4()
                        .child(management_dialog_field("NAME", Input::new(&dialog_name)))
                        .child(management_dialog_field(
                            "DESCRIPTION",
                            Input::new(&dialog_description),
                        )),
                )
        });
        name.read(cx).focus_handle(cx).focus(window);
    }

    fn open_hostname_override_dialog(
        &mut self,
        existing: Option<HostnameOverride>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let hostname = cx.new(|cx| {
            let input = InputState::new(window, cx).placeholder("api.internal");
            if let Some(existing) = existing.as_ref() {
                input.default_value(existing.hostname.clone())
            } else {
                input
            }
        });
        let target = cx.new(|cx| {
            let input = InputState::new(window, cx)
                .placeholder("10.0.0.25, gateway.internal, or https://gateway.internal");
            if let Some(existing) = existing.as_ref() {
                input.default_value(existing.target.clone())
            } else {
                input
            }
        });
        let original_hostname = existing.as_ref().map(|entry| entry.hostname.clone());
        let title = if existing.is_some() {
            "Edit hostname override"
        } else {
            "New hostname override"
        };
        let this = cx.entity().downgrade();
        let dialog_hostname = hostname.clone();
        let dialog_target = target.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let save_this = this.clone();
            let save_hostname = dialog_hostname.clone();
            let save_target = dialog_target.clone();
            let save_original = original_hostname.clone();
            dialog
                .title(title)
                .w(px(480.))
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("Save override"))
                .on_ok(move |_, window, cx| {
                    let hostname = save_hostname.read(cx).value().trim().to_owned();
                    let target = save_target.read(cx).value().trim().to_owned();
                    if hostname.is_empty() || target.is_empty() {
                        return false;
                    }
                    if let Some(this) = save_this.upgrade() {
                        this.update(cx, |this, cx| {
                            let Some(mut settings) = this
                                .server_management
                                .snapshot
                                .as_ref()
                                .and_then(|snapshot| snapshot.request_execution_settings.clone())
                            else {
                                return;
                            };
                            if let Some(original) = save_original.as_deref() {
                                settings
                                    .hostname_overrides
                                    .retain(|entry| entry.hostname != original);
                            }
                            settings.hostname_overrides.push(HostnameOverride {
                                hostname: hostname.clone(),
                                target: target.clone(),
                            });
                            this.run_management_mutation(
                                ManagementMutation::UpdateRequestExecutionSettings { settings },
                                window,
                                cx,
                            );
                        });
                    }
                    true
                })
                .child(
                    v_flex()
                        .gap_4()
                        .child(management_dialog_field(
                            "REQUEST HOSTNAME",
                            Input::new(&dialog_hostname),
                        ))
                        .child(management_dialog_field(
                            "TARGET HOSTNAME OR IP",
                            Input::new(&dialog_target),
                        ))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(
                                    "An IP target keeps the request hostname for HTTP Host and HTTPS SNI. A hostname target replaces both. Prefix the target with http:// or https:// to define its scheme and allow matching request URLs to omit one.",
                                ),
                        ),
                )
        });
        hostname.read(cx).focus_handle(cx).focus(window);
    }

    fn request_delete_hostname_override(
        &mut self,
        hostname: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let delete_this = this.clone();
            let delete_hostname = hostname.clone();
            dialog
                .title("Delete hostname override?")
                .w(px(440.))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete override")
                        .ok_variant(ButtonVariant::Danger),
                )
                .on_ok(move |_, window, cx| {
                    if let Some(this) = delete_this.upgrade() {
                        this.update(cx, |this, cx| {
                            let Some(mut settings) = this
                                .server_management
                                .snapshot
                                .as_ref()
                                .and_then(|snapshot| snapshot.request_execution_settings.clone())
                            else {
                                return;
                            };
                            settings
                                .hostname_overrides
                                .retain(|entry| entry.hostname != delete_hostname);
                            this.run_management_mutation(
                                ManagementMutation::UpdateRequestExecutionSettings { settings },
                                window,
                                cx,
                            );
                        });
                    }
                    true
                })
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "Requests to ‘{hostname}’ will return to normal DNS resolution."
                        )),
                )
        });
    }
}

fn render_user_management(this: &WeakEntity<ApiTester>, cx: &mut App) -> AnyElement {
    discord_views::render_user_management(this, cx)
}

fn render_role_management(this: &WeakEntity<ApiTester>, cx: &mut App) -> AnyElement {
    discord_views::render_role_management(this, cx)
}

fn render_resource_management(this: &WeakEntity<ApiTester>, cx: &mut App) -> AnyElement {
    discord_views::render_resource_management(this, cx)
}

fn management_status_element(
    status: &ServerManagementStatus,
    upstream_id: Option<&str>,
    snapshot_present: bool,
    active_id: Option<&str>,
    cx: &mut App,
) -> Option<AnyElement> {
    if active_id.is_none() {
        return Some(management_empty(
            "Select a connected server to manage its settings.",
            cx,
        ));
    }
    if upstream_id != active_id {
        return Some(management_empty("Loading server settings…", cx));
    }
    match status {
        ServerManagementStatus::Idle | ServerManagementStatus::Loading => {
            Some(management_empty("Loading server settings…", cx))
        }
        ServerManagementStatus::Saving if !snapshot_present => {
            Some(management_empty("Saving changes…", cx))
        }
        ServerManagementStatus::Saving => None,
        ServerManagementStatus::Error(message) => Some(management_empty(message.clone(), cx)),
        ServerManagementStatus::Ready => None,
    }
}

fn management_empty(message: impl Into<SharedString>, cx: &mut App) -> AnyElement {
    div()
        .w_full()
        .min_h(px(120.))
        .flex()
        .items_center()
        .justify_center()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(message.into())
        .into_any_element()
}

fn management_badge(label: impl Into<SharedString>, color: Hsla) -> AnyElement {
    div()
        .px_1p5()
        .py_0p5()
        .rounded_sm()
        .bg(color.opacity(0.12))
        .text_color(color)
        .text_size(px(10.))
        .font_semibold()
        .child(label.into())
        .into_any_element()
}

fn creator_line(creator: Option<&UpstreamUserSummary>, cx: &mut App) -> AnyElement {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(creator_text(creator))
        .into_any_element()
}

fn creator_text(creator: Option<&UpstreamUserSummary>) -> String {
    creator.map_or_else(
        || "Created by an unknown user".to_owned(),
        |creator| format!("Created by {} ({})", creator.display_name, creator.email),
    )
}

fn management_dialog_field(label: &'static str, input: Input) -> AnyElement {
    v_flex()
        .gap_2()
        .child(div().text_xs().font_semibold().child(label))
        .child(input)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;

    use super::*;

    fn permission_keys(keys: &[&str]) -> BTreeSet<String> {
        keys.iter().map(|key| (*key).to_owned()).collect()
    }

    #[test]
    fn shared_history_response_body_decodes_utf8_once_without_extra_copies() {
        let response = SharedHistoryResponse {
            status: 201,
            status_text: "Created".to_owned(),
            http_version: "HTTP/2".to_owned(),
            final_url: "https://api.example.test/widgets/1".to_owned(),
            headers: Vec::new(),
            body_base64: BASE64_STANDARD.encode(br#"{"id":1}"#),
            body_truncated: false,
            content_type: "application/json".to_owned(),
            duration_micros: 1_250,
        };
        assert_eq!(shared_history_response_body(&response), r#"{"id":1}"#);

        // Non-UTF-8 payloads degrade to a byte-count summary.
        let binary = SharedHistoryResponse {
            body_base64: BASE64_STANDARD.encode([0x00u8, 0x01, 0xff]),
            ..response.clone()
        };
        assert_eq!(
            shared_history_response_body(&binary),
            "Binary response body (3 bytes)"
        );

        // Corrupt base64 degrades to an explicit label.
        let corrupt = SharedHistoryResponse {
            body_base64: "%%%".to_owned(),
            ..response
        };
        assert_eq!(
            shared_history_response_body(&corrupt),
            "Invalid shared response body"
        );
    }

    #[test]
    fn role_permission_draft_only_becomes_dirty_until_returned_to_baseline() {
        let mut draft = RolePermissionDraft::new(permission_keys(&["requests.read"]));

        draft.toggle("requests.update");
        assert!(draft.is_dirty());
        assert_eq!(
            draft.selected,
            permission_keys(&["requests.read", "requests.update"])
        );

        draft.toggle("requests.update");
        assert!(!draft.is_dirty());
        assert_eq!(draft.selected, permission_keys(&["requests.read"]));
    }

    #[test]
    fn role_permission_draft_preserves_local_intent_across_realtime_rebase() {
        let mut draft =
            RolePermissionDraft::new(permission_keys(&["requests.read", "requests.update"]));
        draft.toggle("requests.update");
        draft.toggle("requests.execute");

        draft.rebase(permission_keys(&[
            "collections.read",
            "requests.read",
            "requests.update",
        ]));

        assert_eq!(
            draft.selected,
            permission_keys(&["collections.read", "requests.execute", "requests.read",])
        );
        assert!(draft.is_dirty());
    }
}
