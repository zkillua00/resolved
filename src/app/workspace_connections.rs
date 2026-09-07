use super::request_tab_reconciliation::reconcile_restored_request_tabs;
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RequestSaveRoute {
    Local,
    Upstream,
}

fn request_save_route_for_state(
    workspace_writable: bool,
    upstream_available: bool,
    switch_busy: bool,
    sending: bool,
) -> Option<RequestSaveRoute> {
    if switch_busy || sending {
        None
    } else if workspace_writable {
        Some(RequestSaveRoute::Local)
    } else if upstream_available {
        Some(RequestSaveRoute::Upstream)
    } else {
        None
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum WorkspaceSwitchStatus {
    #[default]
    Idle,
    Loading,
    Error(String),
}

impl WorkspaceSwitchStatus {
    pub(super) fn busy(&self) -> bool {
        matches!(self, Self::Loading)
    }
}

pub(super) fn unreachable_upstream_message(server_label: &str) -> String {
    format!(
        "Can't connect to {server_label}. Check that the server is running and the address is correct."
    )
}

fn upstream_switch_error(server_label: &str, error: &str) -> String {
    if error.starts_with("could not reach the server:") {
        unreachable_upstream_message(server_label)
    } else {
        format!("Could not open {server_label}: {error}")
    }
}

pub(super) struct LoadedUpstreamWorkspace {
    pub(super) selected: Option<LoadedUpstreamWorkspaceView>,
    pub(super) summaries: Vec<UpstreamWorkspaceSummary>,
    pub(super) current_user: crate::core::LoginUser,
    pub(super) permission_keys: BTreeSet<String>,
}

pub(super) struct LoadedUpstreamWorkspaceView {
    pub(super) workspace: UpstreamWorkspaceView,
    pub(super) environments: Vec<UpstreamEnvironmentView>,
}

#[derive(Clone)]
pub(super) struct ActiveUpstreamWorkspace {
    pub(super) upstream_id: String,
    pub(super) workspace_id: String,
    pub(super) base_url: url::Url,
}

pub(super) async fn load_upstream_workspace(
    client: &Client,
    base_url: &url::Url,
    bearer_token: &str,
    preferred_workspace_id: Option<&str>,
) -> Result<LoadedUpstreamWorkspace, String> {
    let current_user = get_upstream_user(client, base_url, bearer_token)
        .await
        .map_err(|error| error.to_string())?;
    let permission_keys = current_user.permission_keys();
    let workspaces = if permission_keys.contains(crate::core::WORKSPACES_READ) {
        list_upstream_workspaces(client, base_url, bearer_token)
            .await
            .map_err(|error| error.to_string())?
    } else {
        Vec::new()
    };
    let summaries = workspaces
        .iter()
        .map(UpstreamWorkspaceView::summary)
        .collect::<Vec<_>>();
    let selected_workspace = preferred_workspace_id
        .and_then(|workspace_id| {
            workspaces
                .iter()
                .find(|workspace| workspace.id == workspace_id)
        })
        .or_else(|| workspaces.first())
        .cloned();
    let selected = if let Some(workspace) = selected_workspace {
        let environments = if permission_keys.contains(ENVIRONMENTS_READ) {
            list_upstream_environments(client, base_url, bearer_token, &workspace.id)
                .await
                .map_err(|error| error.to_string())?
        } else {
            Vec::new()
        };
        Some(LoadedUpstreamWorkspaceView {
            workspace,
            environments,
        })
    } else {
        None
    };
    Ok(LoadedUpstreamWorkspace {
        selected,
        summaries,
        current_user,
        permission_keys,
    })
}

impl ApiTester {
    pub(super) fn active_upstream_has_permission(&self, permission: &str) -> bool {
        let WorkspaceProviderId::Upstream { upstream_id, .. } =
            self.workspace_providers.active_id()
        else {
            return false;
        };
        self.settings
            .upstreams
            .server(upstream_id)
            .is_some_and(|profile| profile.has_permission(permission))
    }

    pub(super) fn can_create_collection_content(&self) -> bool {
        self.workspace_writable
            || (!self.workspace_switch_status.busy()
                && self.active_upstream_workspace().is_ok()
                && self.active_upstream_has_permission(COLLECTIONS_CREATE))
    }

    pub(super) fn can_update_collection_content(&self) -> bool {
        self.workspace_writable
            || (!self.workspace_switch_status.busy()
                && self.active_upstream_workspace().is_ok()
                && self.active_upstream_has_permission(COLLECTIONS_UPDATE))
    }

    pub(super) fn can_delete_collection_content(&self) -> bool {
        self.workspace_writable
            || (!self.workspace_switch_status.busy()
                && self.active_upstream_workspace().is_ok()
                && self.active_upstream_has_permission(COLLECTIONS_DELETE))
    }

    pub(super) fn can_create_request_content(&self) -> bool {
        self.workspace_writable
            || (!self.workspace_switch_status.busy()
                && self.active_upstream_workspace().is_ok()
                && self.active_upstream_has_permission(REQUESTS_CREATE))
    }

    pub(super) fn can_update_request_content(&self) -> bool {
        self.workspace_writable
            || (!self.workspace_switch_status.busy()
                && self.active_upstream_workspace().is_ok()
                && self.active_upstream_has_permission(REQUESTS_UPDATE))
    }

    pub(super) fn can_delete_request_content(&self) -> bool {
        self.workspace_writable
            || (!self.workspace_switch_status.busy()
                && self.active_upstream_workspace().is_ok()
                && self.active_upstream_has_permission(REQUESTS_DELETE))
    }

    pub(super) fn request_save_route(&self, save_as: bool) -> Option<RequestSaveRoute> {
        let updating = !save_as
            && self
                .request_tabs
                .active()
                .association()
                .saved_request_id()
                .is_some();
        let upstream_allowed = if updating {
            self.can_update_request_content()
        } else {
            self.can_create_request_content()
        };
        request_save_route_for_state(
            self.workspace_writable,
            !self.workspace_writable && upstream_allowed,
            self.workspace_switch_status.busy(),
            self.sending,
        )
    }

    pub(super) fn can_save_request_content(&self, save_as: bool) -> bool {
        self.request_save_route(save_as).is_some()
    }

    pub(super) fn active_upstream_workspace(&self) -> Result<ActiveUpstreamWorkspace, String> {
        let WorkspaceProviderId::Upstream {
            upstream_id,
            workspace_id,
        } = self.workspace_providers.active_id()
        else {
            return Err("No server workspace is selected.".to_owned());
        };
        let profile = self
            .settings
            .upstreams
            .server(upstream_id)
            .ok_or_else(|| "That server is no longer configured.".to_owned())?;
        if profile.session_expired(Utc::now()) {
            return Err(format!("Log in to {} again.", profile.display_label()));
        }
        let base_url = profile
            .parsed_base_url()
            .ok_or_else(|| "That server URL is invalid.".to_owned())?;
        Ok(ActiveUpstreamWorkspace {
            upstream_id: upstream_id.clone(),
            workspace_id: workspace_id.clone(),
            base_url,
        })
    }

    pub(super) fn open_create_upstream_collection_dialog(
        &mut self,
        collection_id: Option<String>,
        parent_folder_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sending || !self.can_create_collection_content() {
            return;
        }
        let creating_folder = collection_id.is_some();
        let title = if creating_folder {
            "New folder"
        } else {
            "New collection"
        };
        let placeholder = if creating_folder {
            "Folder name"
        } else {
            "Collection name"
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let this = cx.entity().downgrade();
        let dialog_input = input.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let create_this = this.clone();
            let create_input = dialog_input.clone();
            let create_collection_id = collection_id.clone();
            let create_parent_folder_id = parent_folder_id.clone();
            dialog
                .title(title)
                .w(px(440.))
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("Create"))
                .on_ok(move |_, window, cx| {
                    let name = create_input.read(cx).value().trim().to_owned();
                    if name.is_empty() {
                        return false;
                    }
                    if let Some(this) = create_this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.create_collection_on_upstream(
                                create_collection_id.clone(),
                                create_parent_folder_id.clone(),
                                name.clone(),
                                window,
                                cx,
                            );
                        });
                    }
                    true
                })
                .child(Input::new(&dialog_input))
        });
        input.read(cx).focus_handle(cx).focus(window);
    }

    fn create_collection_on_upstream(
        &mut self,
        collection_id: Option<String>,
        parent_folder_id: Option<String>,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = match self.active_upstream_workspace() {
            Ok(target) => target,
            Err(error) => {
                self.workspace_warning = Some(error);
                cx.notify();
                return;
            }
        };
        let parent_collection_id = collection_id.as_ref().map(|collection_id| {
            parent_folder_id
                .clone()
                .unwrap_or_else(|| collection_id.clone())
        });
        self.workspace_switch_generation = self.workspace_switch_generation.wrapping_add(1);
        let generation = self.workspace_switch_generation;
        self.workspace_switch_status = WorkspaceSwitchStatus::Loading;
        let vault = self.credential_vault.clone();
        let client = self.upstream_client.clone();
        let runtime = Arc::clone(&self.runtime);
        let credential_upstream_id = target.upstream_id.clone();
        let task_target = target.clone();
        let task_name = name.clone();
        let task_parent_collection_id = parent_collection_id.clone();
        let task = self.runtime.spawn(async move {
            let credential = runtime
                .spawn_blocking(move || vault.load_upstream(&credential_upstream_id))
                .await
                .map_err(|error| format!("Could not open the saved session: {error}"))?
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Log in to this server again.".to_owned())?;
            if credential.expires_at <= Utc::now() {
                return Err("Log in to this server again.".to_owned());
            }
            create_upstream_collection(
                &client,
                &task_target.base_url,
                credential.bearer_token(),
                &task_target.workspace_id,
                &task_name,
                task_parent_collection_id.as_deref(),
            )
            .await
            .map_err(|error| error.to_string())
        });
        self.workspace_switch_abort_handle = Some(task.abort_handle());
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.workspace_switch_generation != generation {
                    return;
                }
                this.workspace_switch_abort_handle = None;
                match result {
                    Ok(Ok(created)) => this.finish_upstream_collection_create(
                        target,
                        collection_id,
                        parent_folder_id,
                        created,
                        window,
                        cx,
                    ),
                    Ok(Err(error)) => this.fail_remote_workspace_write(
                        format!("The collection could not be created: {error}"),
                        cx,
                    ),
                    Err(error) if error.is_cancelled() => {}
                    Err(error) => this.fail_remote_workspace_write(
                        format!("The collection could not be created: {error}"),
                        cx,
                    ),
                }
            });
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_upstream_collection_create(
        &mut self,
        target: ActiveUpstreamWorkspace,
        collection_id: Option<String>,
        parent_folder_id: Option<String>,
        created: UpstreamCollectionView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let expected_provider = WorkspaceProviderId::Upstream {
            upstream_id: target.upstream_id.clone(),
            workspace_id: target.workspace_id.clone(),
        };
        if self.workspace_providers.active_id() != &expected_provider {
            self.workspace_switch_status = WorkspaceSwitchStatus::Idle;
            return;
        }

        let mut candidate = self.workspace.clone();
        let created_id = created.id.clone();
        let created_name = created.name.clone();
        let created_by = created.created_by.map(Into::into);
        if let Some(collection_id) = collection_id {
            let expected_parent_id = parent_folder_id
                .clone()
                .unwrap_or_else(|| collection_id.clone());
            if created.parent_collection_id.as_deref() != Some(expected_parent_id.as_str()) {
                self.fail_remote_workspace_write(
                    "The server returned the new folder in an unexpected location.".to_owned(),
                    cx,
                );
                return;
            }
            let Some(collection) = candidate
                .collections
                .iter_mut()
                .find(|collection| collection.id == collection_id)
            else {
                self.fail_remote_workspace_write(
                    "The selected collection is no longer available.".to_owned(),
                    cx,
                );
                return;
            };
            collection.folders.push(CollectionFolder {
                id: created_id.clone(),
                name: created_name.clone(),
                created_by,
                parent_folder_id: parent_folder_id.clone(),
            });
            self.selected_collection_id = Some(collection_id.clone());
            self.selected_folder_id = Some(created_id.clone());
            self.expanded_collection_ids.insert(collection_id);
            if let Some(parent_folder_id) = parent_folder_id {
                self.expanded_folder_ids.insert(parent_folder_id);
            }
            self.folder_name
                .update(cx, |input, cx| input.set_value(created_name, window, cx));
        } else {
            if created.parent_collection_id.is_some() {
                self.fail_remote_workspace_write(
                    "The server returned the new collection in an unexpected location.".to_owned(),
                    cx,
                );
                return;
            }
            candidate.collections.push(Collection {
                id: created_id.clone(),
                name: created_name.clone(),
                created_by,
                folders: Vec::new(),
                requests: Vec::new(),
            });
            self.selected_collection_id = Some(created_id.clone());
            self.selected_folder_id = None;
            self.expanded_collection_ids.insert(created_id.clone());
            self.collection_name
                .update(cx, |input, cx| input.set_value(created_name, window, cx));
        }
        if let Err(error) = candidate.validate() {
            self.fail_remote_workspace_write(
                format!("The server returned an invalid collection: {error}"),
                cx,
            );
            return;
        }
        self.replace_workspace(candidate.clone());
        self.workspace_providers
            .register(Arc::new(RemoteWorkspaceProvider::new(
                self.database_store.clone(),
                target.upstream_id,
                target.workspace_id,
                candidate,
            )));
        self.workspace_switch_status = WorkspaceSwitchStatus::Idle;
        self.workspace_warning = None;
        self.request_notice = Some(if self.selected_folder_id.is_some() {
            "Folder created.".to_owned()
        } else {
            "Collection created.".to_owned()
        });
        self.update_active_unsaved_request_tab_location(
            self.selected_collection_id.clone(),
            self.selected_folder_id.clone(),
            cx,
        );
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn save_request_on_upstream(
        &mut self,
        collection_id: String,
        folder_id: Option<String>,
        update_id: Option<String>,
        name: String,
        definition: RequestTemplate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = match self.active_upstream_workspace() {
            Ok(target) => target,
            Err(error) => {
                self.workspace_warning = Some(error);
                cx.notify();
                return;
            }
        };
        let target_collection_id = folder_id.clone().unwrap_or_else(|| collection_id.clone());
        let request_tab_id = self.request_tabs.active_tab_id().clone();
        let request_tab_title = self.request_tabs.active().title().to_owned();
        self.workspace_switch_generation = self.workspace_switch_generation.wrapping_add(1);
        let generation = self.workspace_switch_generation;
        self.workspace_switch_status = WorkspaceSwitchStatus::Loading;
        let vault = self.credential_vault.clone();
        let client = self.upstream_client.clone();
        let runtime = Arc::clone(&self.runtime);
        let credential_upstream_id = target.upstream_id.clone();
        let task_target = target.clone();
        let task_collection_id = target_collection_id.clone();
        let task_update_id = update_id.clone();
        let task_name = name.clone();
        let task_definition = definition.clone();
        let task = self.runtime.spawn(async move {
            let credential = runtime
                .spawn_blocking(move || vault.load_upstream(&credential_upstream_id))
                .await
                .map_err(|error| format!("Could not open the saved session: {error}"))?
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Log in to this server again.".to_owned())?;
            if credential.expires_at <= Utc::now() {
                return Err("Log in to this server again.".to_owned());
            }
            let saved = if let Some(request_id) = task_update_id.as_deref() {
                update_upstream_saved_request(
                    &client,
                    &task_target.base_url,
                    credential.bearer_token(),
                    &task_target.workspace_id,
                    &task_collection_id,
                    request_id,
                    &task_name,
                    &task_definition,
                )
                .await
            } else {
                create_upstream_saved_request(
                    &client,
                    &task_target.base_url,
                    credential.bearer_token(),
                    &task_target.workspace_id,
                    &task_collection_id,
                    &task_name,
                    &task_definition,
                )
                .await
            };
            saved.map_err(|error| error.to_string())
        });
        self.workspace_switch_abort_handle = Some(task.abort_handle());
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.workspace_switch_generation != generation {
                    return;
                }
                this.workspace_switch_abort_handle = None;
                match result {
                    Ok(Ok(saved)) => this.finish_upstream_request_save(
                        target,
                        collection_id,
                        folder_id,
                        target_collection_id,
                        update_id,
                        request_tab_id,
                        request_tab_title,
                        saved,
                        window,
                        cx,
                    ),
                    Ok(Err(error)) => this.fail_remote_workspace_write(
                        format!("The request could not be saved: {error}"),
                        cx,
                    ),
                    Err(error) if error.is_cancelled() => {}
                    Err(error) => this.fail_remote_workspace_write(
                        format!("The request could not be saved: {error}"),
                        cx,
                    ),
                }
            });
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_upstream_request_save(
        &mut self,
        target: ActiveUpstreamWorkspace,
        collection_id: String,
        folder_id: Option<String>,
        target_collection_id: String,
        update_id: Option<String>,
        request_tab_id: RequestTabId,
        request_tab_title: String,
        saved: UpstreamSavedRequestView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let expected_provider = WorkspaceProviderId::Upstream {
            upstream_id: target.upstream_id.clone(),
            workspace_id: target.workspace_id.clone(),
        };
        if self.workspace_providers.active_id() != &expected_provider {
            self.workspace_switch_status = WorkspaceSwitchStatus::Idle;
            return;
        }
        if saved.collection_id != target_collection_id
            || update_id
                .as_deref()
                .is_some_and(|request_id| request_id != saved.id)
        {
            self.fail_remote_workspace_write(
                "The server returned the saved request in an unexpected location.".to_owned(),
                cx,
            );
            return;
        }

        let mut candidate = self.workspace.clone();
        let Some(collection) = candidate
            .collections
            .iter_mut()
            .find(|collection| collection.id == collection_id)
        else {
            self.fail_remote_workspace_write(
                "The selected collection is no longer available.".to_owned(),
                cx,
            );
            return;
        };
        let persisted_template = saved.definition.clone();
        let local_saved = saved.into_local(folder_id.clone());
        let request_id = local_saved.id.clone();
        let saved_name = local_saved.name.clone();
        if let Some(update_id) = update_id {
            let Some(existing) = collection
                .requests
                .iter_mut()
                .find(|request| request.id == update_id)
            else {
                self.fail_remote_workspace_write(
                    "The saved request is no longer available.".to_owned(),
                    cx,
                );
                return;
            };
            *existing = local_saved;
        } else {
            collection.requests.push(local_saved);
        }
        if let Err(error) = candidate.validate() {
            self.fail_remote_workspace_write(
                format!("The server returned an invalid saved request: {error}"),
                cx,
            );
            return;
        }

        self.snapshot_active_request_tab(cx);
        let mut candidate_request_tabs = self.request_tabs.clone();
        let tab_was_saved = candidate_request_tabs.mark_tab_saved(
            &request_tab_id,
            &request_tab_title,
            saved_name.clone(),
            RequestTabAssociation::new(folder_id, Some(collection_id), Some(request_id.clone())),
            persisted_template.clone(),
        );
        let saved_tab_is_active =
            tab_was_saved && candidate_request_tabs.active_tab_id() == &request_tab_id;
        let saved_tab_title = candidate_request_tabs
            .get(&request_tab_id)
            .map(|tab| tab.title().to_owned());
        let request_tabs_error = self
            .workspace_providers
            .active()
            .save_request_tabs(&candidate_request_tabs)
            .err();
        self.replace_workspace(candidate.clone());
        self.request_tabs = candidate_request_tabs;
        self.last_persisted_request_tabs = self.request_tabs.clone();
        self.workspace_providers
            .register(Arc::new(RemoteWorkspaceProvider::new(
                self.database_store.clone(),
                target.upstream_id,
                target.workspace_id,
                candidate,
            )));
        self.workspace_switch_status = WorkspaceSwitchStatus::Idle;
        self.workspace_warning = None;
        self.request_tabs_warning = request_tabs_error
            .map(|error| format!("The request tab could not be remembered: {error}"));
        if saved_tab_is_active {
            self.request_dirty.begin_hydration();
            self.saved_request_name.update(cx, |input, cx| {
                input.set_value(saved_tab_title.unwrap_or(saved_name), window, cx)
            });
            self.request_dirty.end_hydration();
            self.loaded_request_baseline = persisted_template;
            self.detached_request_dirty = false;
            self.request_dirty.clear();
            self.refresh_all_request_dirty_parts(cx);
        }
        self.sync_active_request_tab_identity();
        self.request_notice = None;
        cx.notify();
    }

    pub(super) fn fail_remote_workspace_write(&mut self, message: String, cx: &mut Context<Self>) {
        self.workspace_switch_status = WorkspaceSwitchStatus::Error(message.clone());
        self.workspace_warning = Some(message);
        cx.notify();
    }

    pub(super) fn active_workspace_name(&self) -> String {
        match self.workspace_providers.active_id() {
            WorkspaceProviderId::Local(workspace_id) => self
                .local_workspaces
                .iter()
                .find(|workspace| workspace.id == *workspace_id)
                .map(|workspace| workspace.name.clone())
                .unwrap_or_else(|| "Local workspace".to_owned()),
            WorkspaceProviderId::Upstream {
                upstream_id,
                workspace_id,
            } => self
                .settings
                .upstreams
                .server(upstream_id)
                .and_then(|server| {
                    (server.active_workspace_id.as_deref() == Some(workspace_id.as_str()))
                        .then(|| server.active_workspace())
                        .flatten()
                        .or_else(|| {
                            server
                                .workspaces
                                .iter()
                                .find(|workspace| workspace.id == *workspace_id)
                        })
                })
                .map(|workspace| workspace.name.clone())
                .unwrap_or_else(|| "Server workspace".to_owned()),
        }
    }

    pub(super) fn open_create_local_workspace_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_writable || self.workspace_switch_status.busy() || self.sending {
            return;
        }
        self.workspace_name.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        let input = self.workspace_name.clone();
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let create_this = this.clone();
            let create_input = input.clone();
            dialog
                .title("New workspace")
                .w(px(440.))
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("Create workspace"))
                .on_ok(move |_, window, cx| {
                    let name = create_input.read(cx).value().trim().to_owned();
                    if name.is_empty() {
                        return false;
                    }
                    if let Some(this) = create_this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.create_local_workspace(name.clone(), window, cx);
                        });
                    }
                    true
                })
                .child(
                    v_flex()
                        .debug_selector(|| "local-workspace-create-dialog".to_owned())
                        .gap_2()
                        .child(div().text_sm().child("Workspace name"))
                        .child(Input::new(&input)),
                )
        });
        self.workspace_name.read(cx).focus_handle(cx).focus(window);
    }

    pub(super) fn open_create_upstream_workspace_dialog(
        &mut self,
        upstream_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_writable || self.workspace_switch_status.busy() || self.sending {
            return;
        }
        let Some(profile) = self.settings.upstreams.server(&upstream_id) else {
            self.settings_notice = Some("That server is no longer configured.".to_owned());
            cx.notify();
            return;
        };
        if profile.session_expired(Utc::now()) {
            self.settings_notice = Some(format!(
                "Log in to {} again before creating a workspace.",
                profile.display_label()
            ));
            cx.notify();
            return;
        }
        if !profile.has_permission(WORKSPACES_CREATE) {
            return;
        }

        let title = format!("New workspace on {}", profile.display_label());
        self.workspace_name.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        let input = self.workspace_name.clone();
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let create_this = this.clone();
            let create_input = input.clone();
            let create_upstream_id = upstream_id.clone();
            dialog
                .title(title.clone())
                .w(px(440.))
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("Create workspace"))
                .on_ok(move |_, window, cx| {
                    let name = create_input.read(cx).value().trim().to_owned();
                    if name.is_empty() {
                        return false;
                    }
                    if let Some(this) = create_this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.create_workspace_on_upstream(
                                create_upstream_id.clone(),
                                name.clone(),
                                window,
                                cx,
                            );
                        });
                    }
                    true
                })
                .child(
                    v_flex()
                        .debug_selector(|| "upstream-workspace-create-dialog".to_owned())
                        .gap_2()
                        .child(div().text_sm().child("Workspace name"))
                        .child(Input::new(&input)),
                )
        });
        self.workspace_name.read(cx).focus_handle(cx).focus(window);
    }

    fn create_local_workspace(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.prepare_for_workspace_switch(cx) {
            return;
        }
        match self.database_store.create_local_workspace(&name) {
            Ok(workspace) => {
                let workspace_id = workspace.id.clone();
                self.workspace_providers
                    .register(Arc::new(LocalWorkspaceProvider::new(
                        self.database_store.clone(),
                        workspace_id.clone(),
                    )));
                self.local_workspaces.push(workspace);
                self.switch_to_local_workspace(workspace_id, window, cx);
            }
            Err(error) => {
                self.workspace_switch_status = WorkspaceSwitchStatus::Error(error.to_string());
                self.settings_notice = Some(format!("The workspace could not be created: {error}"));
                cx.notify();
            }
        }
    }

    pub(super) fn switch_to_local_workspace(
        &mut self,
        workspace_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let provider_id = WorkspaceProviderId::Local(workspace_id.clone());
        if self.workspace_providers.active_id() == &provider_id
            && self.settings.upstreams.active_upstream_id.is_none()
        {
            if matches!(
                self.workspace_switch_status,
                WorkspaceSwitchStatus::Error(_)
            ) {
                self.workspace_switch_status = WorkspaceSwitchStatus::Idle;
                self.settings_notice = None;
                cx.notify();
            }
            return;
        }
        if !self.prepare_for_workspace_switch(cx) {
            return;
        }
        if !self.workspace_providers.contains(&provider_id) {
            if !self
                .local_workspaces
                .iter()
                .any(|workspace| workspace.id == workspace_id)
            {
                self.settings_notice = Some("That workspace is no longer available.".to_owned());
                cx.notify();
                return;
            }
            self.workspace_providers
                .register(Arc::new(LocalWorkspaceProvider::new(
                    self.database_store.clone(),
                    workspace_id.clone(),
                )));
        }
        let (workspace, mut request_tabs, workspace_writable, request_tabs_writable) = {
            let provider = match self.workspace_providers.provider(&provider_id) {
                Ok(provider) => provider,
                Err(error) => {
                    self.settings_notice = Some(error.to_string());
                    cx.notify();
                    return;
                }
            };
            let workspace = match provider.load_workspace() {
                Ok(workspace) => workspace,
                Err(error) => {
                    self.settings_notice =
                        Some(format!("The workspace could not be opened: {error}"));
                    cx.notify();
                    return;
                }
            };
            let request_tabs = match provider.load_request_tabs() {
                Ok(request_tabs) => request_tabs,
                Err(error) => {
                    self.settings_notice =
                        Some(format!("Request tabs could not be restored: {error}"));
                    cx.notify();
                    return;
                }
            };
            (
                workspace,
                request_tabs,
                provider.workspace_writable(),
                provider.request_tabs_writable(),
            )
        };
        if reconcile_restored_request_tabs(&mut request_tabs, &workspace, true)
            && request_tabs_writable
            && let Err(error) = self
                .workspace_providers
                .provider(&provider_id)
                .and_then(|provider| provider.save_request_tabs(&request_tabs))
        {
            self.settings_notice = Some(format!("Request tabs could not be repaired: {error}"));
            cx.notify();
            return;
        }

        let prepared_cookie_client = match self.cookie_client_for(&provider_id) {
            Ok(client) => client,
            Err(error) => {
                self.fail_workspace_switch(
                    format!("Could not prepare workspace cookies: {error}"),
                    cx,
                );
                return;
            }
        };
        let mut settings = self.settings.clone();
        settings.upstreams.select_local();
        if let Err(error) = self
            .database_store
            .select_local_workspace_and_save_settings(&workspace_id, &settings)
        {
            self.settings_notice = Some(format!("The workspace could not be selected: {error}"));
            cx.notify();
            return;
        }
        if let Err(error) = self.apply_persisted_settings(settings, false, cx) {
            self.settings_notice = Some(error);
            cx.notify();
            return;
        }
        if !self.activate_loaded_workspace(
            provider_id,
            prepared_cookie_client,
            workspace,
            request_tabs,
            workspace_writable,
            request_tabs_writable,
            window,
            cx,
        ) {
            return;
        }
        // `activate_loaded_workspace` rebuilds `WorkspaceTabs` from the loaded
        // request tabs, so every tool tab (including ServerTools) is closed and
        // the active tab is a request or the welcome page by construction; no
        // explicit tool teardown or management refresh is needed here.
        self.workspace_switch_status = WorkspaceSwitchStatus::Idle;
        self.stop_realtime();
        self.settings_notice = Some(format!("Opened {}.", self.active_workspace_name()));
        cx.notify();
    }

    pub(super) fn switch_to_upstream(
        &mut self,
        upstream_id: String,
        workspace_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.prepare_for_workspace_switch(cx) {
            return;
        }
        let Some(profile) = self.settings.upstreams.server(&upstream_id).cloned() else {
            self.fail_workspace_switch("That server is no longer configured.".to_owned(), cx);
            return;
        };
        if profile.session_expired(Utc::now()) {
            self.fail_workspace_switch(
                format!(
                    "Log in to {} again before opening its workspaces.",
                    profile.display_label()
                ),
                cx,
            );
            return;
        }
        let Some(base_url) = profile.parsed_base_url() else {
            self.fail_workspace_switch("That server URL is invalid.".to_owned(), cx);
            return;
        };
        let profile_label = profile.display_label();

        self.workspace_switch_generation = self.workspace_switch_generation.wrapping_add(1);
        let generation = self.workspace_switch_generation;
        self.workspace_switch_status = WorkspaceSwitchStatus::Loading;
        self.workspace_warning = None;
        self.settings_notice = None;
        let vault = self.credential_vault.clone();
        let client = self.upstream_client.clone();
        let runtime = Arc::clone(&self.runtime);
        let task_upstream_id = upstream_id.clone();
        let preferred_workspace_id = workspace_id.or(profile.active_workspace_id.clone());
        let task = self.runtime.spawn(async move {
            let credential = runtime
                .spawn_blocking(move || vault.load_upstream(&task_upstream_id))
                .await
                .map_err(|error| format!("Could not open the saved session: {error}"))?
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Log in to this server again.".to_owned())?;
            if credential.expires_at <= Utc::now() {
                return Err("Log in to this server again.".to_owned());
            }
            load_upstream_workspace(
                &client,
                &base_url,
                credential.bearer_token(),
                preferred_workspace_id.as_deref(),
            )
            .await
        });
        self.workspace_switch_abort_handle = Some(task.abort_handle());
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.workspace_switch_generation != generation {
                    return;
                }
                this.workspace_switch_abort_handle = None;
                match result {
                    Ok(Ok(loaded)) => this.finish_upstream_switch(upstream_id, loaded, window, cx),
                    Ok(Err(error)) => this
                        .fail_workspace_switch(upstream_switch_error(&profile_label, &error), cx),
                    Err(error) if error.is_cancelled() => {}
                    Err(error) => this.fail_workspace_switch(
                        format!("Could not open {profile_label}: {error}"),
                        cx,
                    ),
                }
            });
        })
        .detach();
    }

    fn create_workspace_on_upstream(
        &mut self,
        upstream_id: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.prepare_for_workspace_switch(cx) {
            return;
        }
        let Some(profile) = self.settings.upstreams.server(&upstream_id).cloned() else {
            self.settings_notice = Some("That server is no longer configured.".to_owned());
            cx.notify();
            return;
        };
        if profile.session_expired(Utc::now()) {
            self.settings_notice = Some(format!(
                "Log in to {} again before creating a workspace.",
                profile.display_label()
            ));
            cx.notify();
            return;
        }
        if !profile.has_permission(WORKSPACES_CREATE) {
            return;
        }
        let Some(base_url) = profile.parsed_base_url() else {
            self.settings_notice = Some("That server URL is invalid.".to_owned());
            cx.notify();
            return;
        };

        self.workspace_switch_generation = self.workspace_switch_generation.wrapping_add(1);
        let generation = self.workspace_switch_generation;
        self.workspace_switch_status = WorkspaceSwitchStatus::Loading;
        let vault = self.credential_vault.clone();
        let client = self.upstream_client.clone();
        let runtime = Arc::clone(&self.runtime);
        let task_upstream_id = upstream_id.clone();
        let mut summaries = profile.workspaces;
        let task = self.runtime.spawn(async move {
            let credential = runtime
                .spawn_blocking(move || vault.load_upstream(&task_upstream_id))
                .await
                .map_err(|error| format!("Could not open the saved session: {error}"))?
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Log in to this server again.".to_owned())?;
            if credential.expires_at <= Utc::now() {
                return Err("Log in to this server again.".to_owned());
            }
            let current_user = get_upstream_user(&client, &base_url, credential.bearer_token())
                .await
                .map_err(|error| format!("The account permissions could not be loaded: {error}"))?;
            let permission_keys = current_user.permission_keys();
            let created =
                create_upstream_workspace(&client, &base_url, credential.bearer_token(), &name)
                    .await
                    .map_err(|error| format!("The workspace could not be created: {error}"))?;
            let created_summary = created.summary();
            if let Some(existing) = summaries
                .iter_mut()
                .find(|workspace| workspace.id == created_summary.id)
            {
                *existing = created_summary;
            } else {
                summaries.push(created_summary);
            }
            let environments = if permission_keys.contains(ENVIRONMENTS_READ) {
                list_upstream_environments(
                    &client,
                    &base_url,
                    credential.bearer_token(),
                    &created.id,
                )
                .await
                .map_err(|error| {
                    format!("The workspace environments could not be loaded: {error}")
                })?
            } else {
                Vec::new()
            };
            Ok(LoadedUpstreamWorkspace {
                selected: Some(LoadedUpstreamWorkspaceView {
                    workspace: created,
                    environments,
                }),
                summaries,
                current_user,
                permission_keys,
            })
        });
        self.workspace_switch_abort_handle = Some(task.abort_handle());
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.workspace_switch_generation != generation {
                    return;
                }
                this.workspace_switch_abort_handle = None;
                match result {
                    Ok(Ok(loaded)) => this.finish_upstream_switch(upstream_id, loaded, window, cx),
                    Ok(Err(error)) => this.fail_workspace_switch(error, cx),
                    Err(error) if error.is_cancelled() => {}
                    Err(error) => this.fail_workspace_switch(
                        format!("The workspace could not be created: {error}"),
                        cx,
                    ),
                }
            });
        })
        .detach();
    }

    pub(super) fn finish_upstream_switch(
        &mut self,
        upstream_id: String,
        loaded: LoadedUpstreamWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut settings = self.settings.clone();
        let Some(profile) = settings
            .upstreams
            .servers
            .iter_mut()
            .find(|profile| profile.id == upstream_id)
        else {
            self.fail_workspace_switch("That server is no longer configured.".to_owned(), cx);
            return;
        };
        let selected_workspace_id = loaded
            .selected
            .as_ref()
            .map(|loaded| loaded.workspace.id.clone());
        profile.user_id.clone_from(&loaded.current_user.id);
        profile.email.clone_from(&loaded.current_user.email);
        profile
            .display_name
            .clone_from(&loaded.current_user.display_name);
        profile.replace_permissions(loaded.permission_keys);
        profile.replace_workspaces(loaded.summaries, selected_workspace_id.clone());
        let Some(selected) = loaded.selected else {
            if let Err(error) = self.database_store.save_app_settings(&settings) {
                self.fail_workspace_switch(
                    format!("The server workspace list could not be saved: {error}"),
                    cx,
                );
            } else if let Err(error) = self.apply_persisted_settings(settings, false, cx) {
                self.fail_workspace_switch(error, cx);
            } else {
                self.fail_workspace_switch(
                    "No workspaces are available on this server.".to_owned(),
                    cx,
                );
            }
            return;
        };
        let workspace_id = selected.workspace.id.clone();
        let workspace_name = selected.workspace.name.clone();
        let active_environment_id = profile
            .active_environment_id(&workspace_id)
            .map(str::to_owned)
            .filter(|environment_id| {
                selected
                    .environments
                    .iter()
                    .any(|environment| environment.id == *environment_id)
            });
        profile.set_active_environment_id(&workspace_id, active_environment_id.as_deref());
        let mut workspace = selected.workspace.into_local_workspace();
        workspace.environments = selected
            .environments
            .into_iter()
            .map(UpstreamEnvironmentView::into_local)
            .collect();
        workspace.active_environment_id = active_environment_id;
        if let Err(error) = workspace.validate() {
            self.fail_workspace_switch(
                format!("The server returned an invalid workspace: {error}"),
                cx,
            );
            return;
        }
        let provider = RemoteWorkspaceProvider::new(
            self.database_store.clone(),
            upstream_id.clone(),
            workspace_id.clone(),
            workspace.clone(),
        );
        let provider_id = provider.id();
        let mut request_tabs = match provider.load_request_tabs() {
            Ok(request_tabs) => request_tabs,
            Err(error) => {
                self.fail_workspace_switch(
                    format!("Request tabs could not be restored: {error}"),
                    cx,
                );
                return;
            }
        };
        if reconcile_restored_request_tabs(&mut request_tabs, &workspace, true)
            && let Err(error) = provider.save_request_tabs(&request_tabs)
        {
            self.fail_workspace_switch(format!("Request tabs could not be repaired: {error}"), cx);
            return;
        }
        let prepared_cookie_client = match self.cookie_client_for(&provider_id) {
            Ok(client) => client,
            Err(error) => {
                self.fail_workspace_switch(
                    format!("Could not prepare workspace cookies: {error}"),
                    cx,
                );
                return;
            }
        };
        if !settings.upstreams.select(&upstream_id) {
            self.fail_workspace_switch("That server is no longer configured.".to_owned(), cx);
            return;
        }
        if let Err(error) = self.database_store.save_app_settings(&settings) {
            self.fail_workspace_switch(
                format!("The workspace selection could not be saved: {error}"),
                cx,
            );
            return;
        }
        if let Err(error) = self.apply_persisted_settings(settings, false, cx) {
            self.fail_workspace_switch(error, cx);
            return;
        }
        let workspace_writable = provider.workspace_writable();
        let request_tabs_writable = provider.request_tabs_writable();
        self.workspace_providers.register(Arc::new(provider));
        if !self.activate_loaded_workspace(
            provider_id,
            prepared_cookie_client,
            workspace,
            request_tabs,
            workspace_writable,
            request_tabs_writable,
            window,
            cx,
        ) {
            return;
        }
        self.workspace_switch_status = WorkspaceSwitchStatus::Idle;
        self.settings_notice = Some(format!("Opened {workspace_name}."));
        self.start_realtime_for_active_upstream(window, cx);
        // The workspace tabs were reset by `activate_loaded_workspace`; opening
        // ServerTools/RequestProxy afterwards loads its own management snapshot.
        cx.notify();
    }

    fn prepare_for_workspace_switch(&mut self, cx: &mut Context<Self>) -> bool {
        if self.sending || self.workspace_switch_status.busy() {
            return false;
        }
        if !self.flush_local_state(cx) {
            self.settings_notice = Some(
                "The current editor buffers could not be persisted before switching workspaces."
                    .to_owned(),
            );
            cx.notify();
            return false;
        }
        true
    }

    fn fail_workspace_switch(&mut self, message: String, cx: &mut Context<Self>) {
        self.workspace_switch_status = WorkspaceSwitchStatus::Error(message);
        self.settings_notice = None;
        cx.notify();
    }

    pub(super) fn cookie_client_for(
        &self,
        provider_id: &WorkspaceProviderId,
    ) -> Result<(Arc<CookieJar>, Client), RequestError> {
        let jar = Arc::new(CookieJar::open(
            self.credential_vault.clone(),
            provider_id.to_string(),
        ));
        let client = build_client_with_cookie_jar(jar.clone())?;
        Ok((jar, client))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn activate_loaded_workspace(
        &mut self,
        provider_id: WorkspaceProviderId,
        prepared_cookie_client: (Arc<CookieJar>, Client),
        workspace: Workspace,
        request_tabs: RequestTabs,
        workspace_writable: bool,
        request_tabs_writable: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let (cookie_jar, client) = prepared_cookie_client;
        if self.workspace_providers.active_id() != &provider_id {
            self.stop_mcp_websocket();
        }
        if let Err(error) = self.workspace_providers.switch(provider_id) {
            self.settings_notice = Some(error.to_string());
            cx.notify();
            return false;
        }
        if let Some(abort_handle) = self.profile_history_abort_handle.take() {
            abort_handle.abort();
        }
        self.profile_history_generation = self.profile_history_generation.wrapping_add(1);
        self.server_management.reset_profile_history();
        let workspace_tabs = WorkspaceTabs::from_request_tabs(&request_tabs);
        let active_workspace_tab = workspace_tabs.active_tab(&request_tabs);
        let visible_workspace_tabs = workspace_tabs.visible_tabs(&request_tabs);
        let panes = PaneRoot::from_tabs(
            visible_workspace_tabs.clone(),
            visible_workspace_tabs
                .iter()
                .position(|tab| *tab == active_workspace_tab)
                .unwrap_or(0),
        );
        let selected_collection_id = request_tabs
            .active()
            .association()
            .collection_id()
            .and_then(|id| workspace.collection(id))
            .map(|collection| collection.id.clone())
            .or_else(|| {
                workspace
                    .collections
                    .first()
                    .map(|collection| collection.id.clone())
            });
        let selected_folder_id = request_tabs
            .active()
            .association()
            .folder_id()
            .filter(|folder_id| {
                selected_collection_id
                    .as_deref()
                    .and_then(|id| workspace.collection(id))
                    .is_some_and(|collection| collection.folder(folder_id).is_some())
            })
            .map(ToOwned::to_owned);
        let expanded_folder_ids = selected_collection_id
            .as_deref()
            .and_then(|collection_id| workspace.collection(collection_id))
            .and_then(|collection| {
                selected_folder_id
                    .as_deref()
                    .and_then(|folder_id| collection.folder_path_ids(folder_id).ok())
            })
            .unwrap_or_default()
            .into_iter()
            .collect();
        let selected_environment_id = workspace.active_environment_id.clone().or_else(|| {
            workspace
                .environments
                .first()
                .map(|environment| environment.id.clone())
        });
        let collection_name = selected_collection_id
            .as_deref()
            .and_then(|id| workspace.collection(id))
            .map(|collection| collection.name.clone())
            .unwrap_or_default();
        let environment_name = selected_environment_id
            .as_deref()
            .and_then(|id| workspace.environment(id))
            .map(|environment| environment.name.clone())
            .unwrap_or_default();

        self.hide_preview(cx);
        self.client = client;
        self.cookie_jar = cookie_jar;
        self.replace_workspace(workspace);
        self.workspace_warning = None;
        self.workspace_writable = workspace_writable;
        self.request_tabs = request_tabs;
        self.last_persisted_request_tabs = self.request_tabs.clone();
        self.request_tabs_writable = request_tabs_writable;
        self.request_tabs_warning = None;
        self.request_tabs_persist_task = None;
        self.request_tab_runtime.clear();
        self.request_tab_context_target = None;
        self.workspace_tabs = workspace_tabs;
        self.panes = panes;
        self.pane_editors.clear();
        self.selected_collection_id = selected_collection_id;
        self.selected_folder_id = selected_folder_id;
        self.selected_environment_id = selected_environment_id;
        self.expanded_collection_ids = self
            .workspace
            .collections
            .iter()
            .map(|collection| collection.id.clone())
            .collect();
        self.expanded_folder_ids = expanded_folder_ids;
        self.active_saved_request_id = None;
        self.renaming_collection_id = None;
        self.renaming_folder_id = None;
        self.pending_delete = None;
        self.collection_name
            .update(cx, |input, cx| input.set_value(collection_name, window, cx));
        self.environment_name.update(cx, |input, cx| {
            input.set_value(environment_name, window, cx)
        });
        self.environment_variables = Self::environment_rows(
            self.selected_environment_id
                .as_deref()
                .and_then(|id| self.workspace.environment(id)),
            window,
            cx,
        );
        update_script_variable_catalog(&self.script_variable_catalog, &self.workspace);
        self.template_variable_catalog
            .borrow_mut()
            .replace_environment(self.workspace.active_environment());
        self.snippet_editor = Self::create_snippet_editor_session(
            &self.snippets,
            true,
            Rc::clone(&self.script_variable_catalog),
            self.typescript_service.clone(),
            window,
            cx,
        );
        self.restore_active_request_tab(window, cx);
        true
    }

    pub(super) fn restore_selected_upstream(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(upstream_id) = self.settings.upstreams.active_upstream_id.clone() else {
            return;
        };
        let workspace_id = self
            .settings
            .upstreams
            .server(&upstream_id)
            .and_then(|profile| profile.active_workspace_id.clone());
        self.switch_to_upstream(upstream_id, workspace_id, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_save_route_keeps_local_and_upstream_persistence_reachable() {
        assert_eq!(
            request_save_route_for_state(true, false, false, false),
            Some(RequestSaveRoute::Local)
        );
        assert_eq!(
            request_save_route_for_state(false, true, false, false),
            Some(RequestSaveRoute::Upstream)
        );
        assert_eq!(
            request_save_route_for_state(false, false, false, false),
            None
        );
        assert_eq!(request_save_route_for_state(false, true, true, false), None);
        assert_eq!(request_save_route_for_state(false, true, false, true), None);
    }

    #[test]
    fn workspace_switch_errors_hide_transport_details() {
        assert_eq!(
            upstream_switch_error(
                "resolved.example.com",
                "could not reach the server: connection refused"
            ),
            "Can't connect to resolved.example.com. Check that the server is running and the address is correct."
        );
        assert_eq!(
            upstream_switch_error("resolved.example.com", "a valid bearer token is required"),
            "Could not open resolved.example.com: a valid bearer token is required"
        );
    }
}
