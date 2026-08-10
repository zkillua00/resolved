use super::request_tab_reconciliation::reconcile_restored_request_tabs;
use super::*;

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

struct LoadedUpstreamWorkspace {
    selected: Option<UpstreamWorkspaceView>,
    summaries: Vec<UpstreamWorkspaceSummary>,
}

impl ApiTester {
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
        self.activate_loaded_workspace(
            provider_id,
            workspace,
            request_tabs,
            workspace_writable,
            request_tabs_writable,
            window,
            cx,
        );
        self.workspace_switch_status = WorkspaceSwitchStatus::Idle;
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
            self.settings_notice = Some("That server is no longer configured.".to_owned());
            cx.notify();
            return;
        };
        if profile.session_expired(Utc::now()) {
            self.settings_notice = Some(format!(
                "Log in to {} again before opening its workspaces.",
                profile.display_label()
            ));
            cx.notify();
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
            let workspaces =
                list_upstream_workspaces(&client, &base_url, credential.bearer_token())
                    .await
                    .map_err(|error| error.to_string())?;
            let summaries = workspaces
                .iter()
                .map(UpstreamWorkspaceView::summary)
                .collect::<Vec<_>>();
            let selected = preferred_workspace_id
                .as_deref()
                .and_then(|workspace_id| {
                    workspaces
                        .iter()
                        .find(|workspace| workspace.id == workspace_id)
                })
                .or_else(|| workspaces.first())
                .cloned();
            Ok(LoadedUpstreamWorkspace {
                selected,
                summaries,
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
                    Ok(Err(error)) => {
                        this.workspace_switch_status = WorkspaceSwitchStatus::Error(error.clone());
                        this.settings_notice = Some(error);
                        cx.notify();
                    }
                    Err(error) if error.is_cancelled() => {}
                    Err(error) => {
                        let message = format!("The workspace could not be opened: {error}");
                        this.workspace_switch_status =
                            WorkspaceSwitchStatus::Error(message.clone());
                        this.settings_notice = Some(message);
                        cx.notify();
                    }
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
            Ok(LoadedUpstreamWorkspace {
                selected: Some(created),
                summaries,
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

    fn finish_upstream_switch(
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
            .map(|workspace| workspace.id.clone());
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
        let workspace_id = selected.id.clone();
        let workspace_name = selected.name.clone();
        let workspace = selected.into_local_workspace();
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
        self.activate_loaded_workspace(
            provider_id,
            workspace,
            request_tabs,
            workspace_writable,
            request_tabs_writable,
            window,
            cx,
        );
        self.workspace_switch_status = WorkspaceSwitchStatus::Idle;
        self.settings_notice = Some(format!("Opened {workspace_name}."));
        cx.notify();
    }

    fn prepare_for_workspace_switch(&mut self, cx: &mut Context<Self>) -> bool {
        if self.sending || self.workspace_switch_status.busy() {
            return false;
        }
        if !self.flush_local_state(cx) {
            self.settings_notice =
                Some("Save or discard the current changes before switching workspaces.".to_owned());
            cx.notify();
            return false;
        }
        true
    }

    fn fail_workspace_switch(&mut self, message: String, cx: &mut Context<Self>) {
        self.workspace_switch_status = WorkspaceSwitchStatus::Error(message.clone());
        self.settings_notice = Some(message);
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    fn activate_loaded_workspace(
        &mut self,
        provider_id: WorkspaceProviderId,
        workspace: Workspace,
        request_tabs: RequestTabs,
        workspace_writable: bool,
        request_tabs_writable: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self.workspace_providers.switch(provider_id) {
            self.settings_notice = Some(error.to_string());
            return;
        }
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
        self.workspace = workspace;
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
            &self.workspace,
            self.workspace_writable,
            Rc::clone(&self.script_variable_catalog),
            self.typescript_service.clone(),
            window,
            cx,
        );
        self.restore_active_request_tab(window, cx);
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
