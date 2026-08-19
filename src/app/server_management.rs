use std::collections::BTreeSet;

use gpui_component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_component::switch::Switch;
use zeroize::Zeroizing;

use crate::core::{
    COLLECTIONS_ASSIGN_USERS, HISTORY_READ_OTHERS, ManagementRole, ManagementUser,
    ROLES_ASSIGN_PERMISSIONS, ROLES_CREATE, ROLES_UPDATE, SharedHistoryEntry, USERS_ASSIGN_ROLES,
    USERS_CREATE, USERS_UPDATE, UpstreamCollectionView, UpstreamManagementSnapshot,
    UpstreamSavedRequestView, UpstreamUserSummary, UpstreamWorkspaceView, WORKSPACES_ASSIGN_USERS,
    create_management_role, create_management_user, list_shared_history, load_upstream_management,
    replace_management_collection_users, replace_management_role_permissions,
    replace_management_user_roles, replace_management_workspace_users, update_management_role,
    update_management_user,
};

use super::*;

mod discord_views;
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

impl ServerManagementStatus {
    fn busy(&self) -> bool {
        matches!(self, Self::Loading | Self::Saving)
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct ServerManagementState {
    pub upstream_id: Option<String>,
    pub status: ServerManagementStatus,
    pub snapshot: Option<UpstreamManagementSnapshot>,
    selected_user_id: Option<String>,
    selected_profile_id: Option<String>,
    selected_profile_history_id: Option<String>,
    profile_history_status: ProfileHistoryStatus,
    profile_history: Vec<SharedHistoryEntry>,
    selected_role_id: Option<String>,
    selected_resource: Option<ManagementResourceSelection>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ManagementResourceSelection {
    Workspace(String),
    Collection(String),
    Request(String),
}

impl ServerManagementState {
    pub(super) fn reset_profile_history(&mut self) {
        self.profile_history_status = ProfileHistoryStatus::Idle;
        self.profile_history.clear();
        self.selected_profile_history_id = None;
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

        self.server_management = ServerManagementState {
            upstream_id: Some(upstream_id.clone()),
            status: ServerManagementStatus::Loading,
            snapshot: None,
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
            let _ = this.update_in(cx, |this, _, cx| {
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
                cx.notify();
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
            let _ = this.update_in(cx, |this, _, cx| {
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
                cx.notify();
            });
        })
        .detach();
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
                        this.server_management.profile_history = entries;
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
    state: &ServerManagementState,
    active_id: Option<&str>,
    cx: &mut App,
) -> Option<AnyElement> {
    if active_id.is_none() {
        return Some(management_empty(
            "Select a connected server to manage its settings.",
            cx,
        ));
    }
    if state.upstream_id.as_deref() != active_id {
        return Some(management_empty("Loading server settings…", cx));
    }
    match &state.status {
        ServerManagementStatus::Idle | ServerManagementStatus::Loading => {
            Some(management_empty("Loading server settings…", cx))
        }
        ServerManagementStatus::Saving if state.snapshot.is_none() => {
            Some(management_empty("Saving changes…", cx))
        }
        ServerManagementStatus::Saving => None,
        ServerManagementStatus::Error(message) => Some(management_empty(message, cx)),
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
