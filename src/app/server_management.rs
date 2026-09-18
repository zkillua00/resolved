use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use gpui::{ListAlignment, ListState};
use gpui_component::group_box::GroupBoxVariant;
use gpui_component::setting::{SettingGroup, SettingItem, SettingPage, Settings as SettingsView};
use gpui_component::switch::Switch;
use zeroize::Zeroizing;

use crate::core::{
    COLLECTIONS_ASSIGN_USERS, HISTORY_READ_OTHERS, ManagementProxy, ManagementRole, ManagementUser,
    PROXIES_ASSIGN, PROXIES_CREATE, PROXIES_DELETE, PROXIES_UPDATE, ProxyAssignment,
    ProxyScopeKind, ROLES_ASSIGN_PERMISSIONS, ROLES_CREATE, ROLES_UPDATE, SharedHistoryEntry,
    SharedHistoryResponse, USERS_ASSIGN_ROLES, USERS_CREATE, USERS_UPDATE, UpstreamCollectionView,
    UpstreamManagementSnapshot, UpstreamSavedRequestView, UpstreamUserSummary,
    UpstreamWorkspaceView, WORKSPACES_ASSIGN_USERS, create_management_proxy,
    create_management_role, create_management_user, delete_management_proxy, list_shared_history,
    load_upstream_management, replace_management_collection_users,
    replace_management_proxy_assignments, replace_management_proxy_exclusions,
    replace_management_role_permissions, replace_management_user_roles,
    replace_management_workspace_users, update_management_proxy, update_management_role,
    update_management_user,
};

use super::*;

pub(super) mod activity_views;
mod discord_views;
mod execution_limit_views;
mod network_views;
mod profile_detail;
mod profile_filters;
mod profile_views;
mod proxy_rule_editor;
mod proxy_scope_views;

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
    profile_filters: profile_filters::ProfileFiltersState,
    profile_lists: profile_views::ProfileLists,
    selected_profile_id: Option<String>,
    selected_profile_history_id: Option<String>,
    profile_detail: profile_detail::ProfileDetailState,
    profile_history_status: ProfileHistoryStatus,
    profile_history: Rc<Vec<SharedHistoryEntry>>,
    profile_history_query: crate::core::SharedHistoryQuery,
    profile_history_syncing: bool,
    profile_history_view_visible: bool,
    profile_history_refresh_pending: bool,
    profile_history_revision: u64,
    change_log: ActivityLogFeed,
    audit_log: ActivityLogFeed,
    change_log_list: ListState,
    audit_log_list: ListState,
    selected_role_id: Option<String>,
    role_permission_drafts: BTreeMap<String, RolePermissionDraft>,
    selected_resource: Option<ManagementResourceSelection>,
    realtime_refresh_pending: bool,
    proxy_workspace: network_views::RequestProxyState,
    proxy_rule_editor: proxy_rule_editor::ProxyRuleEditorState,
    proxy_scope_editor: proxy_scope_views::ProxyScopeEditorState,
    execution_limits: execution_limit_views::ExecutionLimitState,
}

impl Default for ServerManagementState {
    fn default() -> Self {
        Self {
            upstream_id: None,
            status: ServerManagementStatus::default(),
            snapshot: None,
            selected_user_id: None,
            profile_filters: profile_filters::ProfileFiltersState::default(),
            profile_lists: profile_views::ProfileLists::default(),
            selected_profile_id: None,
            selected_profile_history_id: None,
            profile_detail: profile_detail::ProfileDetailState::default(),
            profile_history_status: ProfileHistoryStatus::Idle,
            profile_history: Rc::default(),
            profile_history_query: crate::core::SharedHistoryQuery::default(),
            profile_history_syncing: false,
            profile_history_view_visible: false,
            profile_history_refresh_pending: false,
            profile_history_revision: 0,
            change_log: ActivityLogFeed::default(),
            audit_log: ActivityLogFeed::default(),
            change_log_list: ListState::new(0, ListAlignment::Top, px(200.)),
            audit_log_list: ListState::new(0, ListAlignment::Top, px(200.)),
            selected_role_id: None,
            role_permission_drafts: BTreeMap::new(),
            selected_resource: None,
            realtime_refresh_pending: false,
            proxy_workspace: network_views::RequestProxyState::default(),
            proxy_rule_editor: proxy_rule_editor::ProxyRuleEditorState::default(),
            proxy_scope_editor: proxy_scope_views::ProxyScopeEditorState::default(),
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
        self.profile_history_revision = self.profile_history_revision.wrapping_add(1);
        self.profile_history_syncing = false;
        self.profile_history_refresh_pending = false;
        self.profile_history_status = ProfileHistoryStatus::Idle;
        self.profile_history = Rc::default();
        self.profile_detail = profile_detail::ProfileDetailState::default();
        self.profile_lists.reset_history_scroll();
        self.selected_profile_history_id = None;
    }

    fn set_profile_history(&mut self, entries: Vec<SharedHistoryEntry>) {
        // Read selection at completion, not when the request started: the user
        // can inspect another entry while a background refresh is in flight.
        self.selected_profile_history_id = self
            .selected_profile_history_id
            .take()
            .filter(|selected| entries.iter().any(|entry| &entry.id == selected))
            .or_else(|| entries.first().map(|entry| entry.id.clone()));
        self.profile_detail.retain_entry(&entries, self.selected_profile_history_id.as_deref());
        self.profile_history = Rc::new(entries);
    }

    fn coalesce_profile_history_refresh(&mut self) -> bool {
        if self.profile_history_syncing {
            self.profile_history_refresh_pending = true;
            true
        } else {
            false
        }
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
        self.profile_lists.set_members(&snapshot.profiles);
        self.reconcile_role_permission_drafts(&snapshot);
        self.proxy_rule_editor.reconcile(&snapshot);
        self.proxy_scope_editor.reconcile(&snapshot);
        if !self.proxy_rule_editor.is_editing() && !self.proxy_scope_editor.is_editing() {
            self.proxy_workspace.reconcile(&snapshot);
        }
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
    CreateProxy {
        name: String,
    },
    RenameProxy {
        proxy_id: String,
        name: String,
    },
    UpdateProxyRules {
        proxy_id: String,
        rules: Vec<HostnameOverride>,
    },
    DeleteProxy {
        proxy_id: String,
    },
    ReplaceProxyAssignments {
        proxy_id: String,
        assignments: Vec<ProxyAssignment>,
    },
    ReplaceProxyExclusions {
        proxy_id: String,
        excluded_user_ids: Vec<String>,
        excluded_role_ids: Vec<String>,
    },
}

struct ManagementMutationResult {
    notice: String,
    created_proxy_id: Option<String>,
}

impl ManagementMutation {
    async fn execute(
        self,
        client: &Client,
        base_url: &url::Url,
        bearer_token: &str,
    ) -> Result<ManagementMutationResult, String> {
        let mut created_proxy_id = None;
        let notice: Result<String, String> = match self {
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
            Self::CreateProxy { name } => {
                let proxy = create_management_proxy(client, base_url, bearer_token, &name, &[])
                    .await
                    .map_err(|error| error.to_string())?;
                created_proxy_id = Some(proxy.id);
                Ok(format!("Created proxy {}.", proxy.name))
            }
            Self::RenameProxy { proxy_id, name } => {
                let proxy = update_management_proxy(
                    client,
                    base_url,
                    bearer_token,
                    &proxy_id,
                    Some(&name),
                    None,
                )
                .await
                .map_err(|error| error.to_string())?;
                Ok(format!("Renamed proxy to {}.", proxy.name))
            }
            Self::UpdateProxyRules { proxy_id, rules } => {
                let proxy = update_management_proxy(
                    client,
                    base_url,
                    bearer_token,
                    &proxy_id,
                    None,
                    Some(&rules),
                )
                .await
                .map_err(|error| error.to_string())?;
                Ok(format!("Updated rules for {}.", proxy.name))
            }
            Self::DeleteProxy { proxy_id } => {
                let proxy = delete_management_proxy(client, base_url, bearer_token, &proxy_id)
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(format!("Deleted proxy {}.", proxy.name))
            }
            Self::ReplaceProxyAssignments {
                proxy_id,
                assignments,
            } => {
                let proxy = replace_management_proxy_assignments(
                    client,
                    base_url,
                    bearer_token,
                    &proxy_id,
                    &assignments,
                )
                .await
                .map_err(|error| error.to_string())?;
                Ok(format!("Updated assignments for {}.", proxy.name))
            }
            Self::ReplaceProxyExclusions {
                proxy_id,
                excluded_user_ids,
                excluded_role_ids,
            } => {
                let proxy = replace_management_proxy_exclusions(
                    client,
                    base_url,
                    bearer_token,
                    &proxy_id,
                    &excluded_user_ids,
                    &excluded_role_ids,
                )
                .await
                .map_err(|error| error.to_string())?;
                Ok(format!("Updated exclusions for {}.", proxy.name))
            }
        };
        Ok(ManagementMutationResult {
            notice: notice?,
            created_proxy_id,
        })
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
        if matches!(self.server_management.status, ServerManagementStatus::Ready)
            && matches!(
                self.server_management.profile_history_status,
                ProfileHistoryStatus::Idle
            )
        {
            self.ensure_profile_history_loaded(window, cx);
        }
        self.ensure_proxy_workspace_inputs(window, cx);
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
        // Early errors must preserve edits just like an ordinary failed refresh.
        // A different server, however, must start with completely separate state.
        if self.server_management.upstream_id.as_deref() != Some(&upstream_id) {
            self.server_management = ServerManagementState {
                upstream_id: Some(upstream_id.clone()),
                ..ServerManagementState::default()
            };
        }
        if profile.session_expired(Utc::now()) {
            self.server_management.status = ServerManagementStatus::Error(
                "Log in to this server again to manage it.".to_owned(),
            );
            self.server_management.snapshot = None;
            cx.notify();
            return;
        }
        let Some(base_url) = profile.parsed_base_url() else {
            self.server_management.status =
                ServerManagementStatus::Error("The server URL is invalid.".to_owned());
            self.server_management.snapshot = None;
            cx.notify();
            return;
        };
        // A different server was reset above. Preserve same-server inputs,
        // drafts and history in place, without copying body-bearing entries.
        self.server_management.status = ServerManagementStatus::Loading;
        self.ensure_proxy_workspace_inputs(window, cx);
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
                        this.sync_upstream_profile_permissions(
                            &upstream_id,
                            &snapshot.current_user,
                            cx,
                        );
                        this.server_management.status = ServerManagementStatus::Ready;
                        this.server_management.set_snapshot(snapshot);
                        this.ensure_profile_history_loaded(window, cx);
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
            self.finish_proxy_editor_save(false, Some("Select a server first.".to_owned()));
            self.settings_notice = Some("Select a server first.".to_owned());
            cx.notify();
            return;
        };
        let Some(base_url) = profile.parsed_base_url() else {
            self.finish_proxy_editor_save(false, Some("The server URL is invalid.".to_owned()));
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
            let outcome = mutation
                .execute(&client, &base_url, credential.bearer_token())
                .await?;
            let snapshot = load_upstream_management(&client, &base_url, credential.bearer_token())
                .await
                .map_err(|error| error.to_string())?;
            Ok::<_, String>((outcome, snapshot))
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
                    Ok(Ok((outcome, snapshot))) => {
                        this.finish_proxy_editor_save(true, None);
                        if let Some(proxy_id) = &outcome.created_proxy_id {
                            this.server_management
                                .proxy_workspace
                                .select_proxy_by_id(proxy_id, snapshot.proxies.as_deref());
                        }
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
                        this.ensure_profile_history_loaded(window, cx);
                        this.settings_notice = Some(outcome.notice);
                    }
                    Ok(Err(error)) => {
                        this.server_management.status = ServerManagementStatus::Ready;
                        this.finish_proxy_editor_save(false, Some(error.clone()));
                        this.settings_notice = Some(error);
                    }
                    Err(error) if error.is_cancelled() => return,
                    Err(error) => {
                        this.server_management.status = ServerManagementStatus::Ready;
                        let error = format!("The server change could not be saved: {error}");
                        this.finish_proxy_editor_save(false, Some(error.clone()));
                        this.settings_notice = Some(error);
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

    fn finish_proxy_editor_save(&mut self, success: bool, error: Option<String>) {
        if let Some((proxy_id, hostname)) = self
            .server_management
            .proxy_rule_editor
            .finish_save(success)
        {
            self.server_management.proxy_workspace.selected_proxy_id = Some(proxy_id);
            self.server_management
                .proxy_workspace
                .selected_rule_hostname = Some(hostname);
        }
        self.server_management
            .proxy_scope_editor
            .finish_save(success);
        if let Some(error) = error {
            self.server_management
                .proxy_rule_editor
                .set_save_error(error.clone());
            self.server_management
                .proxy_scope_editor
                .set_save_error(error);
        }
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
                move |_, _, cx| {
                    profile_views::hide_profiles(&this, cx);
                    render_user_management(&this, cx)
                },
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
                move |_, _, cx| {
                    profile_views::hide_profiles(&this, cx);
                    render_role_management(&this, cx)
                },
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
                move |_, window, cx| profile_views::render_profiles(&this, window, cx),
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
                move |_, window, cx| {
                    profile_views::hide_profiles(&this, cx);
                    activity_views::render_change_log(&this, window, cx)
                },
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
                move |_, window, cx| {
                    profile_views::hide_profiles(&this, cx);
                    activity_views::render_audit_log(&this, window, cx)
                },
            )))
    }

    pub(super) fn load_profile_history(
        &mut self,
        user_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.load_profile_history_selecting(user_id, false, window, cx);
    }

    fn ensure_profile_history_loaded(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.profile_history_view_active() {
            self.server_management.profile_history_refresh_pending = true;
            return;
        }
        let Some(user_id) = self.server_management.selected_profile_id.clone() else {
            return;
        };
        if matches!(
            self.server_management.profile_history_status,
            ProfileHistoryStatus::Idle
        ) {
            self.load_profile_history(user_id, window, cx);
        } else {
            self.refresh_profile_history_realtime(user_id, window, cx);
        }
    }

    pub(super) fn refresh_profile_history_realtime(
        &mut self,
        user_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.server_management.is_history_visible_for(&user_id) {
            return;
        }
        if !self.profile_history_view_active() {
            // A metadata invalidation is enough while hidden. Catch up with the
            // current query when Profiles is shown, including after reconnect.
            self.server_management.profile_history_refresh_pending = true;
            return;
        }
        if self.server_management.coalesce_profile_history_refresh() {
            return;
        }
        self.load_profile_history_selecting(user_id, true, window, cx);
    }

    fn profile_history_view_active(&self) -> bool {
        self.server_management.profile_history_view_visible
            && matches!(self.workspace_tabs.active(), ActiveWorkspaceTab::ServerTools)
    }

    pub(super) fn catch_up_profile_history_realtime(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(user_id) = self.server_management.selected_profile_id.clone() {
            self.refresh_profile_history_realtime(user_id, window, cx);
        }
    }

    fn load_profile_history_selecting(
        &mut self,
        user_id: String,
        background: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self.server_management.snapshot.as_ref() else {
            return;
        };
        // Workspace switches can precede the next management snapshot load.
        // Never combine the previous server's profile with the new credentials.
        if !matches!(
            self.workspace_providers.active_id(),
            WorkspaceProviderId::Upstream { upstream_id, .. }
                if self.server_management.upstream_id.as_ref() == Some(upstream_id)
        ) {
            return;
        }
        if user_id != snapshot.current_user.id && !snapshot.has_permission(HISTORY_READ_OTHERS) {
            if let Some(abort_handle) = self.profile_history_abort_handle.take() {
                abort_handle.abort();
            }
            self.profile_history_generation = self.profile_history_generation.wrapping_add(1);
            self.server_management.selected_profile_id = Some(user_id);
            self.server_management.reset_profile_history();
            self.server_management.profile_history_status = ProfileHistoryStatus::Error(
                "You do not have permission to view this member's history.".to_owned(),
            );
            cx.notify();
            return;
        }
        let Ok(target) = self.active_upstream_workspace() else {
            if let Some(abort_handle) = self.profile_history_abort_handle.take() {
                abort_handle.abort();
            }
            self.profile_history_generation = self.profile_history_generation.wrapping_add(1);
            self.server_management.reset_profile_history();
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
        if !background {
            self.server_management.reset_profile_history();
            self.server_management.profile_history_status = ProfileHistoryStatus::Loading;
        }
        self.server_management.profile_history_syncing = true;
        self.server_management.profile_history_refresh_pending = false;
        let revision = self.server_management.profile_history_revision;
        let query = self.server_management.profile_history_query.clone();
        let request_query = query.clone();
        let workspace_id = target.workspace_id.clone();
        let request_upstream_id = target.upstream_id.clone();

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
            for attempt in 0..3 {
                let result = list_shared_history(
                    &client,
                    &target.base_url,
                    credential.bearer_token(),
                    &target.workspace_id,
                    &selected_user_id,
                    &request_query,
                )
                .await;
                let retryable = match &result {
                    Err(crate::core::UpstreamManagementError::Transport(_)) => true,
                    Err(crate::core::UpstreamManagementError::Rejected { status, .. }) => {
                        status.is_server_error()
                    }
                    _ => false,
                };
                if !retryable || attempt == 2 {
                    return result.map_err(|error| error.to_string());
                }
                tokio::time::sleep(Duration::from_millis(300 * (attempt + 1))).await;
            }
            unreachable!("the final history attempt always returns")
        });
        self.profile_history_abort_handle = Some(task.abort_handle());
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.profile_history_generation != generation
                    || this.server_management.profile_history_revision != revision
                    || this.server_management.profile_history_query != query
                    || this.server_management.selected_profile_id.as_deref()
                        != Some(user_id.as_str())
                {
                    return;
                }
                this.profile_history_abort_handle = None;
                this.server_management.profile_history_syncing = false;
                if !this.active_upstream_workspace().is_ok_and(|target| {
                    target.upstream_id == request_upstream_id && target.workspace_id == workspace_id
                }) || !this.server_management.is_history_visible_for(&user_id)
                {
                    this.server_management.reset_profile_history();
                    cx.notify();
                    return;
                }
                let pending =
                    std::mem::take(&mut this.server_management.profile_history_refresh_pending);
                match result {
                    Ok(Ok(entries)) => {
                        this.server_management.set_profile_history(entries);
                        this.server_management.profile_history_status = ProfileHistoryStatus::Ready;
                    }
                    Ok(Err(error)) => {
                        this.server_management.set_profile_history(Vec::new());
                        this.server_management.selected_profile_history_id = None;
                        this.server_management.profile_history_status =
                            ProfileHistoryStatus::Error(error);
                    }
                    Err(error) if error.is_cancelled() => return,
                    Err(error) => {
                        this.server_management.set_profile_history(Vec::new());
                        this.server_management.selected_profile_history_id = None;
                        this.server_management.profile_history_status = ProfileHistoryStatus::Error(
                            format!("Shared history could not be loaded: {error}"),
                        );
                    }
                }
                if pending {
                    this.refresh_profile_history_realtime(user_id, window, cx);
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
                move |_, _, cx| {
                    profile_views::hide_profiles(&this, cx);
                    render_resource_management(&this, cx)
                },
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

    fn management_proxy(&self, proxy_id: &str) -> Option<&ManagementProxy> {
        self.server_management
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.proxies.as_ref())
            .and_then(|proxies| proxies.iter().find(|proxy| proxy.id == proxy_id))
    }

    fn open_proxy_name_dialog(
        &mut self,
        existing: Option<ManagementProxy>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = cx.new(|cx| {
            let input = InputState::new(window, cx).placeholder("Internal routing");
            if let Some(existing) = existing.as_ref() {
                input.default_value(existing.name.clone())
            } else {
                input
            }
        });
        let proxy_id = existing.as_ref().map(|proxy| proxy.id.clone());
        let title = if existing.is_some() {
            "Rename proxy"
        } else {
            "New proxy"
        };
        let this = cx.entity().downgrade();
        let dialog_name = name.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let save_this = this.clone();
            let save_name = dialog_name.clone();
            let save_proxy_id = proxy_id.clone();
            dialog
                .title(title)
                .w(px(440.))
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("Save proxy"))
                .on_ok(move |_, window, cx| {
                    let name = save_name.read(cx).value().trim().to_owned();
                    if name.is_empty() {
                        return false;
                    }
                    if let Some(this) = save_this.upgrade() {
                        this.update(cx, |this, cx| {
                            let mutation = match save_proxy_id.clone() {
                                Some(proxy_id) => {
                                    ManagementMutation::RenameProxy { proxy_id, name }
                                }
                                None => ManagementMutation::CreateProxy { name },
                            };
                            this.run_management_mutation(mutation, window, cx);
                        });
                    }
                    true
                })
                .child(
                    v_flex()
                        .gap_4()
                        .child(management_dialog_field("PROXY NAME", Input::new(&dialog_name)))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(
                                    "A proxy is a named set of host override rules. It only takes effect where it is assigned: server-wide, or to a workspace, collection, or request.",
                                ),
                        ),
                )
        });
        name.read(cx).focus_handle(cx).focus(window);
    }

    fn request_delete_proxy(
        &mut self,
        proxy_id: String,
        proxy_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let delete_this = this.clone();
            let delete_proxy_id = proxy_id.clone();
            dialog
                .title("Delete proxy?")
                .w(px(440.))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete proxy")
                        .ok_variant(ButtonVariant::Danger),
                )
                .on_ok(move |_, window, cx| {
                    if let Some(this) = delete_this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.run_management_mutation(
                                ManagementMutation::DeleteProxy {
                                    proxy_id: delete_proxy_id.clone(),
                                },
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
                            "‘{proxy_name}’ and its rules, assignments, and exclusions will be removed. Requests fall through to the next applicable proxy rule, or normal DNS if none matches."
                        )),
                )
        });
    }

    fn request_delete_proxy_rule(
        &mut self,
        proxy_id: String,
        hostname: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let delete_this = this.clone();
            let delete_proxy_id = proxy_id.clone();
            let delete_hostname = hostname.clone();
            dialog
                .title("Delete host override rule?")
                .w(px(440.))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete rule")
                        .ok_variant(ButtonVariant::Danger),
                )
                .on_ok(move |_, window, cx| {
                    if let Some(this) = delete_this.upgrade() {
                        this.update(cx, |this, cx| {
                            let Some(proxy) = this.management_proxy(&delete_proxy_id) else {
                                return;
                            };
                            let mut rules = proxy.rules.clone();
                            rules.retain(|entry| entry.hostname != delete_hostname);
                            this.run_management_mutation(
                                ManagementMutation::UpdateProxyRules {
                                    proxy_id: delete_proxy_id.clone(),
                                    rules,
                                },
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
                            "Requests to ‘{hostname}’ will fall through to less specific proxies or normal DNS resolution."
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

    fn history_entry(id: &str) -> SharedHistoryEntry {
        serde_json::from_value(serde_json::json!({
            "id": id, "created_at": "2026-01-01T00:00:00Z",
            "request": {
                "method": "GET", "url": "https://example.test",
                "headers": [], "body": "", "body_mode": "none",
                "raw_body_language": "", "body_fields": [], "body_truncated": false
            },
            "response": null
        }))
        .unwrap()
    }

    #[test]
    fn profile_history_refresh_coalesces_without_clearing_visible_selection() {
        let mut state = ServerManagementState::default();
        state.set_profile_history(vec![history_entry("first"), history_entry("second")]);
        state.profile_history_status = ProfileHistoryStatus::Ready;
        state.profile_history_syncing = true;
        for _ in 0..20 {
            assert!(state.coalesce_profile_history_refresh());
        }
        assert!(state.profile_history_refresh_pending);
        assert_eq!(state.profile_history.len(), 2);
        state.selected_profile_history_id = Some("second".into());
        state.set_profile_history(vec![history_entry("new"), history_entry("second")]);
        assert_eq!(state.selected_profile_history_id.as_deref(), Some("second"));
        assert!(std::mem::take(&mut state.profile_history_refresh_pending));
        assert!(!state.profile_history_refresh_pending);
        state.profile_history_syncing = false;
        assert!(!state.coalesce_profile_history_refresh());
        state.set_profile_history(vec![history_entry("new")]);
        assert_eq!(state.selected_profile_history_id.as_deref(), Some("new"));
    }

    #[test]
    fn profile_history_reset_invalidates_inflight_results_and_clears_all_data() {
        let mut state = ServerManagementState::default();
        state.set_profile_history(vec![history_entry("entry")]);
        state.profile_history_syncing = true;
        state.profile_history_refresh_pending = true;
        state.profile_history_query.method = "POST".into();
        let revision = state.profile_history_revision;
        state.reset_profile_history();
        assert_ne!(state.profile_history_revision, revision);
        assert!(!state.profile_history_syncing);
        assert!(!state.profile_history_refresh_pending);
        assert!(state.profile_history.is_empty());
        assert!(state.selected_profile_history_id.is_none());
        assert_eq!(state.profile_history_query.method, "POST");
    }

    fn permission_keys(keys: &[&str]) -> BTreeSet<String> {
        keys.iter().map(|key| (*key).to_owned()).collect()
    }

    #[test]
    fn profile_history_permission_revocation_invalidates_running_refresh() {
        let mut state = ServerManagementState::default();
        state.selected_profile_id = Some("other".into());
        state.set_profile_history(vec![history_entry("private")]);
        state.profile_history_status = ProfileHistoryStatus::Ready;
        state.profile_history_syncing = true;
        let revision = state.profile_history_revision;
        state.set_snapshot(UpstreamManagementSnapshot {
            current_user: ManagementUser {
                id: "self".into(),
                email: "self@example.test".into(),
                display_name: "Self".into(),
                active: true,
                roles: Vec::new(),
                created_by: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
            profiles: vec![crate::core::ProfileView {
                id: "other".into(),
                email: "other@example.test".into(),
                display_name: "Other".into(),
                active: true,
            }],
            users: None,
            roles: None,
            permissions: None,
            workspaces: None,
            request_execution_settings: None,
            proxies: None,
        });
        assert_ne!(state.profile_history_revision, revision);
        assert!(!state.is_history_visible_for("other"));
        assert!(state.profile_history.is_empty());
        assert!(!state.profile_history_syncing);
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
