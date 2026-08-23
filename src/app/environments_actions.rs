use super::*;

pub(super) enum UpstreamEnvironmentMutation {
    Create {
        name: String,
    },
    Save {
        baseline: Box<Environment>,
        draft: Box<Environment>,
    },
    Delete {
        environment_id: String,
    },
}

struct UpstreamEnvironmentMutationResult {
    environments: Vec<UpstreamEnvironmentView>,
    created_environment_id: Option<String>,
    mutation_error: Option<String>,
}

impl ApiTester {
    pub(super) fn can_create_environment_content(&self) -> bool {
        self.workspace_writable
            || (!self.workspace_switch_status.busy()
                && self.active_upstream_has_permission(ENVIRONMENTS_READ)
                && self.active_upstream_has_permission(ENVIRONMENTS_CREATE))
    }

    pub(super) fn can_update_environment_definition(&self) -> bool {
        self.workspace_writable
            || (!self.workspace_switch_status.busy()
                && self.active_upstream_has_permission(ENVIRONMENTS_READ)
                && self.active_upstream_has_permission(ENVIRONMENTS_UPDATE))
    }

    pub(super) fn can_delete_environment_content(&self) -> bool {
        self.workspace_writable
            || (!self.workspace_switch_status.busy()
                && self.active_upstream_has_permission(ENVIRONMENTS_READ)
                && self.active_upstream_has_permission(ENVIRONMENTS_DELETE))
    }

    pub(super) fn can_update_environment_values_content(&self) -> bool {
        self.workspace_writable
            || (!self.workspace_switch_status.busy()
                && self.active_upstream_has_permission(ENVIRONMENTS_READ)
                && self.active_upstream_has_permission(ENVIRONMENT_VALUES_UPDATE))
    }

    pub(super) fn can_mutate_environment_content(&self) -> bool {
        self.can_update_environment_definition() || self.can_update_environment_values_content()
    }

    pub(super) fn can_select_environment(&self) -> bool {
        self.workspace_writable
            || (self.settings_writable
                && matches!(
                    self.workspace_providers.active_id(),
                    WorkspaceProviderId::Upstream { .. }
                ))
    }

    pub(super) fn active_environment_editor_is_dirty(&self, cx: &App) -> bool {
        self.workspace.active_environment_id == self.selected_environment_id
            && self.environment_editor_is_dirty(cx)
    }

    pub(super) fn can_save_environment_editor(&self, cx: &App) -> bool {
        let Some(baseline) = self
            .selected_environment_id
            .as_deref()
            .and_then(|id| self.workspace.environment(id))
        else {
            return false;
        };
        let Ok(draft) = self.environment_editor_draft(baseline, cx) else {
            return false;
        };
        self.can_apply_environment_draft(baseline, &draft)
    }

    fn can_apply_environment_draft(&self, baseline: &Environment, draft: &Environment) -> bool {
        if self.workspace_writable {
            return true;
        }
        let definition_changed = baseline.name != draft.name
            || draft.variables.iter().any(|variable| {
                baseline
                    .variables
                    .iter()
                    .find(|candidate| candidate.id == variable.id)
                    .is_none_or(|previous| {
                        previous.key != variable.key
                            || previous.enabled != variable.enabled
                            || previous.secret != variable.secret
                    })
            });
        let definition_deleted = baseline.variables.iter().any(|variable| {
            !draft
                .variables
                .iter()
                .any(|candidate| candidate.id == variable.id)
        });
        let value_changed = draft.variables.iter().any(|variable| {
            baseline
                .variables
                .iter()
                .find(|candidate| candidate.id == variable.id)
                .is_some_and(|previous| previous.value != variable.value)
        });
        (!definition_changed || self.can_update_environment_definition())
            && (!definition_deleted || self.can_delete_environment_content())
            && (!value_changed || self.can_update_environment_values_content())
    }

    pub(super) fn environment_rows(
        environment: Option<&Environment>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<EnvironmentVariableRow> {
        environment
            .map(|environment| {
                environment
                    .variables
                    .iter()
                    .map(|variable| {
                        let variable_id = variable.id.clone();
                        let key = variable.key.clone();
                        let value = variable.value.clone();
                        let key_state = cx.new(|cx| {
                            InputState::new(window, cx)
                                .placeholder("Variable")
                                .default_value(key)
                        });
                        let value_state = cx.new(|cx| {
                            InputState::new(window, cx)
                                .placeholder("Value")
                                .default_value(value)
                                .masked(variable.secret)
                        });
                        let key_row_id = variable_id.clone();
                        let key_subscription = cx.subscribe_in(
                            &key_state,
                            window,
                            move |this, _, event, window, cx| {
                                if matches!(event, InputEvent::PressEnter { .. })
                                    && let Some(row) = this
                                        .environment_variables
                                        .iter()
                                        .find(|row| row.id == key_row_id)
                                {
                                    row.value.read(cx).focus_handle(cx).focus(window);
                                }
                            },
                        );
                        let value_row_id = variable_id.clone();
                        let value_subscription = cx.subscribe_in(
                            &value_state,
                            window,
                            move |this, _, event, window, cx| {
                                if matches!(event, InputEvent::PressEnter { .. }) {
                                    this.focus_next_environment_row(&value_row_id, window, cx);
                                }
                            },
                        );
                        EnvironmentVariableRow {
                            id: variable_id,
                            key: key_state,
                            value: value_state,
                            enabled: variable.enabled,
                            secret: variable.secret,
                            _subscriptions: vec![key_subscription, value_subscription],
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn push_environment_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = format!("draft-variable-{}", self.next_variable_row_id);
        self.next_variable_row_id = self.next_variable_row_id.wrapping_add(1);
        let key = cx.new(|cx| InputState::new(window, cx).placeholder("Variable"));
        let value = cx.new(|cx| InputState::new(window, cx).placeholder("Value"));
        let key_row_id = id.clone();
        let key_subscription = cx.subscribe_in(&key, window, move |this, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. })
                && let Some(row) = this
                    .environment_variables
                    .iter()
                    .find(|row| row.id == key_row_id)
            {
                row.value.read(cx).focus_handle(cx).focus(window);
            }
        });
        let value_row_id = id.clone();
        let value_subscription =
            cx.subscribe_in(&value, window, move |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.focus_next_environment_row(&value_row_id, window, cx);
                }
            });
        self.environment_variables.push(EnvironmentVariableRow {
            id,
            key,
            value,
            enabled: true,
            secret: false,
            _subscriptions: vec![key_subscription, value_subscription],
        });
        cx.notify();
    }

    pub(super) fn focus_next_environment_row(
        &mut self,
        row_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .environment_variables
            .iter()
            .position(|row| row.id == row_id)
        else {
            return;
        };
        if index + 1 == self.environment_variables.len() {
            self.push_environment_row(window, cx);
        }
        if let Some(input) = self
            .environment_variables
            .get(index + 1)
            .map(|row| row.key.clone())
        {
            input.read(cx).focus_handle(cx).focus(window);
        }
    }

    pub(super) fn duplicate_environment_row(
        &mut self,
        row_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .environment_variables
            .iter()
            .position(|row| row.id == row_id)
        else {
            return;
        };
        let key = self.environment_variables[index]
            .key
            .read(cx)
            .value()
            .to_string();
        let value = self.environment_variables[index]
            .value
            .read(cx)
            .unmask_value()
            .to_string();
        let enabled = self.environment_variables[index].enabled;
        let secret = self.environment_variables[index].secret;

        self.push_environment_row(window, cx);
        let Some(mut duplicate) = self.environment_variables.pop() else {
            return;
        };
        duplicate.enabled = enabled;
        duplicate.secret = secret;
        duplicate
            .key
            .update(cx, |input, cx| input.set_value(key, window, cx));
        duplicate.value.update(cx, |input, cx| {
            input.set_value(value, window, cx);
            input.set_masked(secret, window, cx);
        });
        let input = duplicate.key.clone();
        self.environment_variables.insert(index + 1, duplicate);
        input.read(cx).focus_handle(cx).focus(window);
        cx.notify();
    }

    pub(super) fn reload_environment_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let environment = self
            .selected_environment_id
            .as_deref()
            .and_then(|id| self.workspace.environment(id))
            .cloned();
        let name = environment
            .as_ref()
            .map(|environment| environment.name.clone())
            .unwrap_or_default();
        self.environment_name
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.environment_variables = Self::environment_rows(environment.as_ref(), window, cx);
    }

    pub(super) fn create_environment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_create_environment_content() || self.sending {
            return;
        }
        if self.environment_editor_is_dirty(cx) {
            self.workspace_warning =
                Some("Save the current environment before creating another.".to_owned());
            cx.notify();
            return;
        }
        let name = unique_name(
            "Environment",
            self.workspace
                .environments
                .iter()
                .map(|environment| environment.name.as_str()),
        );
        if !self.workspace_writable {
            self.mutate_environment_on_upstream(
                UpstreamEnvironmentMutation::Create { name },
                window,
                cx,
            );
            return;
        }
        let mut candidate = self.workspace.clone();
        match candidate.create_environment(name) {
            Ok(id) => {
                let result = candidate.set_active_environment(Some(&id));
                if result.is_ok() && self.commit_workspace(candidate).is_ok() {
                    self.selected_environment_id = Some(id);
                    self.reload_environment_editor(window, cx);
                    self.refresh_variable_intelligence(cx);
                    self.sidebar_tab = SidebarTab::Environments;
                } else if let Err(error) = result {
                    self.workspace_warning = Some(error.to_string());
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(super) fn select_environment(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.environment(&id).is_none() {
            return;
        }
        if self.selected_environment_id.as_deref() == Some(&id) {
            self.sidebar_tab = SidebarTab::Environments;
            cx.notify();
            return;
        }
        if self.environment_editor_is_dirty(cx) {
            self.workspace_warning =
                Some("Save the current environment before switching.".to_owned());
            self.sidebar_tab = SidebarTab::Environments;
            cx.notify();
            return;
        }
        self.selected_environment_id = Some(id);
        self.reload_environment_editor(window, cx);
        self.sidebar_tab = SidebarTab::Environments;
        cx.notify();
    }

    pub(super) fn environment_editor_is_dirty(&self, cx: &App) -> bool {
        let Some(environment) = self
            .selected_environment_id
            .as_deref()
            .and_then(|id| self.workspace.environment(id))
        else {
            return false;
        };
        if !input_text_equals(&self.environment_name, environment.name.as_str(), cx) {
            return true;
        }
        if self.environment_variables.len() != environment.variables.len() {
            return true;
        }
        self.environment_variables
            .iter()
            .zip(&environment.variables)
            .any(|(row, variable)| {
                row.id != variable.id
                    || !input_text_equals(&row.key, variable.key.as_str(), cx)
                    || !input_text_equals(&row.value, variable.value.as_str(), cx)
                    || row.enabled != variable.enabled
                    || row.secret != variable.secret
            })
    }

    pub(super) fn activate_environment(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if !self.can_select_environment() || self.sending {
            return;
        }
        let mut candidate = self.workspace.clone();
        match candidate.set_active_environment(id.as_deref()) {
            Ok(()) => {
                if self.workspace_writable {
                    if self.commit_workspace(candidate).is_ok() {
                        self.refresh_variable_intelligence(cx);
                    }
                } else if self
                    .persist_upstream_active_environment(id.as_deref(), cx)
                    .is_ok()
                {
                    self.replace_active_remote_workspace(candidate);
                    self.workspace_warning = None;
                    self.refresh_variable_intelligence(cx);
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(super) fn save_environment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending || !self.can_mutate_environment_content() {
            return;
        }
        let Some(environment_id) = self.selected_environment_id.clone() else {
            return;
        };
        let baseline = match self.workspace.environment(&environment_id).cloned() {
            Some(environment) => environment,
            None => return,
        };
        let draft = match self.environment_editor_draft(&baseline, cx) {
            Ok(environment) => environment,
            Err(error) => {
                self.workspace_warning = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        if !self.can_apply_environment_draft(&baseline, &draft) {
            return;
        }

        if !self.workspace_writable {
            self.mutate_environment_on_upstream(
                UpstreamEnvironmentMutation::Save {
                    baseline: Box::new(baseline),
                    draft: Box::new(draft),
                },
                window,
                cx,
            );
            return;
        }

        let mut candidate = self.workspace.clone();
        if let Err(error) = candidate.rename_environment(&environment_id, draft.name.clone()) {
            self.workspace_warning = Some(error.to_string());
            cx.notify();
            return;
        }
        let Some(environment) = candidate
            .environments
            .iter_mut()
            .find(|environment| environment.id == environment_id)
        else {
            return;
        };
        environment.variables = draft.variables;
        if let Err(error) = candidate.validate() {
            self.workspace_warning = Some(error.to_string());
        } else if self.commit_workspace(candidate).is_ok() {
            self.reload_environment_editor(window, cx);
            self.refresh_variable_intelligence(cx);
        }
        cx.notify();
    }

    pub(super) fn revert_environment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_environment_editor(window, cx);
        self.workspace_warning = None;
        cx.notify();
    }

    pub(super) fn open_environment_delete_dialog(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_delete_environment_content() || self.sending {
            return;
        }
        let Some(environment) = self.workspace.environment(&id) else {
            return;
        };
        let environment_name = environment.name.clone();
        let active = self.workspace.active_environment_id.as_deref() == Some(id.as_str());
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let this_for_ok = this.clone();
            let id_for_ok = id.clone();
            dialog
                .title("Delete environment?")
                .w(px(460.))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete environment")
                        .ok_variant(ButtonVariant::Danger),
                )
                .on_ok(move |_, window, cx| {
                    let Some(this) = this_for_ok.upgrade() else {
                        return true;
                    };
                    this.update(cx, |this, cx| {
                        this.delete_environment(id_for_ok.clone(), window, cx);
                    });
                    true
                })
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "“{environment_name}” and all of its variables will be permanently removed."
                                )),
                        )
                        .when(active, |this| {
                            this.child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .bg(cx.theme().warning.opacity(0.1))
                                    .text_xs()
                                    .text_color(cx.theme().warning)
                                    .child(
                                        "This is the active environment. Requests will switch to no environment.",
                                    ),
                            )
                        }),
                )
        });
    }

    pub(super) fn delete_environment(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_delete_environment_content() {
            return;
        }
        if !self.workspace_writable {
            self.mutate_environment_on_upstream(
                UpstreamEnvironmentMutation::Delete { environment_id: id },
                window,
                cx,
            );
            return;
        }
        let deleting_selected = self.selected_environment_id.as_deref() == Some(id.as_str());
        let mut candidate = self.workspace.clone();
        match candidate.remove_environment(&id) {
            Ok(_) => {
                if self.commit_workspace(candidate).is_ok() {
                    if deleting_selected
                        || self
                            .selected_environment_id
                            .as_deref()
                            .is_some_and(|id| self.workspace.environment(id).is_none())
                    {
                        self.selected_environment_id = self
                            .workspace
                            .environments
                            .first()
                            .map(|environment| environment.id.clone());
                        self.reload_environment_editor(window, cx);
                    }
                    self.refresh_variable_intelligence(cx);
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
    }

    fn environment_editor_draft(
        &self,
        baseline: &Environment,
        cx: &App,
    ) -> Result<Environment, WorkspaceMutationError> {
        let mut draft = Environment::new(self.environment_name.read(cx).value().to_string())?;
        draft.id = baseline.id.clone();
        draft.created_by = baseline.created_by.clone();
        draft.variables.reserve(self.environment_variables.len());
        for row in &self.environment_variables {
            draft.add_variable(
                row.key.read(cx).value().to_string(),
                row.value.read(cx).unmask_value().to_string(),
                row.enabled,
                row.secret,
            )?;
            if let Some(existing) = baseline
                .variables
                .iter()
                .find(|variable| variable.id == row.id)
            {
                let Some(variable) = draft.variables.last_mut() else {
                    continue;
                };
                variable.id = row.id.clone();
                variable.created_by = existing.created_by.clone();
            }
        }
        Ok(draft)
    }

    fn persist_upstream_active_environment(
        &mut self,
        environment_id: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let WorkspaceProviderId::Upstream {
            upstream_id,
            workspace_id,
        } = self.workspace_providers.active_id()
        else {
            return Err("No server workspace is selected.".to_owned());
        };
        let mut settings = self.settings.clone();
        let profile = settings
            .upstreams
            .servers
            .iter_mut()
            .find(|profile| profile.id == *upstream_id)
            .ok_or_else(|| "That server is no longer configured.".to_owned())?;
        profile.set_active_environment_id(workspace_id, environment_id);
        self.commit_settings(settings, false, cx)
    }

    fn replace_active_remote_workspace(&mut self, workspace: Workspace) {
        let WorkspaceProviderId::Upstream {
            upstream_id,
            workspace_id,
        } = self.workspace_providers.active_id().clone()
        else {
            return;
        };
        self.replace_workspace(workspace.clone());
        self.workspace_providers
            .register(Arc::new(RemoteWorkspaceProvider::new(
                self.database_store.clone(),
                upstream_id,
                workspace_id,
                workspace,
            )));
    }

    pub(super) fn mutate_environment_on_upstream(
        &mut self,
        mutation: UpstreamEnvironmentMutation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let allowed = match &mutation {
            UpstreamEnvironmentMutation::Create { .. } => self.can_create_environment_content(),
            UpstreamEnvironmentMutation::Save { baseline, draft } => {
                self.can_apply_environment_draft(baseline, draft)
            }
            UpstreamEnvironmentMutation::Delete { .. } => self.can_delete_environment_content(),
        };
        if !allowed {
            return;
        }
        let target = match self.active_upstream_workspace() {
            Ok(target) => target,
            Err(error) => {
                self.workspace_warning = Some(error);
                cx.notify();
                return;
            }
        };
        self.workspace_switch_generation = self.workspace_switch_generation.wrapping_add(1);
        let generation = self.workspace_switch_generation;
        self.workspace_switch_status = WorkspaceSwitchStatus::Loading;
        let vault = self.credential_vault.clone();
        let client = self.upstream_client.clone();
        let runtime = Arc::clone(&self.runtime);
        let credential_upstream_id = target.upstream_id.clone();
        let task_target = target.clone();
        let success_notice = match &mutation {
            UpstreamEnvironmentMutation::Create { .. } => "Environment created.",
            UpstreamEnvironmentMutation::Save { .. } => "Environment saved.",
            UpstreamEnvironmentMutation::Delete { .. } => "Environment deleted.",
        }
        .to_owned();
        let failure_prefix = match &mutation {
            UpstreamEnvironmentMutation::Create { .. } => "The environment could not be created",
            UpstreamEnvironmentMutation::Save { .. } => "The environment could not be saved",
            UpstreamEnvironmentMutation::Delete { .. } => "The environment could not be deleted",
        };
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

            let mutation_result = match mutation {
                UpstreamEnvironmentMutation::Create { name } => create_upstream_environment(
                    &client,
                    &task_target.base_url,
                    credential.bearer_token(),
                    &task_target.workspace_id,
                    &name,
                )
                .await
                .map(|created| Some(created.id))
                .map_err(|error| format!("{failure_prefix}: {error}")),
                UpstreamEnvironmentMutation::Save { baseline, draft } => save_upstream_environment(
                    &client,
                    &task_target.base_url,
                    credential.bearer_token(),
                    &task_target.workspace_id,
                    baseline.as_ref(),
                    draft.as_ref(),
                )
                .await
                .map(|()| None)
                .map_err(|error| format!("{failure_prefix}: {error}")),
                UpstreamEnvironmentMutation::Delete { environment_id } => {
                    delete_upstream_environment(
                        &client,
                        &task_target.base_url,
                        credential.bearer_token(),
                        &task_target.workspace_id,
                        &environment_id,
                    )
                    .await
                    .map(|()| None)
                    .map_err(|error| format!("{failure_prefix}: {error}"))
                }
            };
            let environments = list_upstream_environments(
                &client,
                &task_target.base_url,
                credential.bearer_token(),
                &task_target.workspace_id,
            )
            .await
            .map_err(|error| match &mutation_result {
                Ok(_) => format!("The environment list could not be refreshed: {error}"),
                Err(mutation_error) => format!(
                    "{mutation_error}. The environment list could not be refreshed: {error}"
                ),
            })?;
            let (created_environment_id, mutation_error) = match mutation_result {
                Ok(created_environment_id) => (created_environment_id, None),
                Err(error) => (None, Some(error)),
            };
            Ok(UpstreamEnvironmentMutationResult {
                environments,
                created_environment_id,
                mutation_error,
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
                    Ok(Ok(result)) => this.finish_upstream_environment_mutation(
                        target,
                        result,
                        success_notice,
                        window,
                        cx,
                    ),
                    Ok(Err(error)) => this.fail_remote_workspace_write(error, cx),
                    Err(error) if error.is_cancelled() => {}
                    Err(error) => this.fail_remote_workspace_write(
                        format!("The environment could not be changed: {error}"),
                        cx,
                    ),
                }
            });
        })
        .detach();
    }

    fn finish_upstream_environment_mutation(
        &mut self,
        target: ActiveUpstreamWorkspace,
        result: UpstreamEnvironmentMutationResult,
        success_notice: String,
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
        if result
            .environments
            .iter()
            .any(|environment| environment.workspace_id != target.workspace_id)
        {
            self.fail_remote_workspace_write(
                "The server returned an environment from another workspace.".to_owned(),
                cx,
            );
            return;
        }

        let mut candidate = self.workspace.clone();
        candidate.environments = result
            .environments
            .into_iter()
            .map(UpstreamEnvironmentView::into_local)
            .collect();
        let current_active = candidate.active_environment_id.clone();
        let desired_active = result
            .created_environment_id
            .clone()
            .filter(|id| candidate.environment(id).is_some())
            .or_else(|| current_active.filter(|id| candidate.environment(id).is_some()));
        candidate.active_environment_id = desired_active.clone();
        let mut persistence_error = None;
        if candidate.active_environment_id != self.workspace.active_environment_id
            && let Err(error) =
                self.persist_upstream_active_environment(desired_active.as_deref(), cx)
        {
            persistence_error = Some(error);
        }
        if let Err(error) = candidate.validate() {
            self.fail_remote_workspace_write(
                format!("The server returned invalid environments: {error}"),
                cx,
            );
            return;
        }

        self.selected_environment_id = result
            .created_environment_id
            .filter(|id| candidate.environment(id).is_some())
            .or_else(|| {
                self.selected_environment_id
                    .clone()
                    .filter(|id| candidate.environment(id).is_some())
            })
            .or_else(|| {
                candidate
                    .environments
                    .first()
                    .map(|environment| environment.id.clone())
            });
        self.replace_active_remote_workspace(candidate);
        self.reload_environment_editor(window, cx);
        self.refresh_variable_intelligence(cx);

        let warning = result.mutation_error.or(persistence_error);
        if let Some(warning) = warning {
            self.workspace_switch_status = WorkspaceSwitchStatus::Error(warning.clone());
            self.workspace_warning = Some(warning);
        } else {
            self.workspace_switch_status = WorkspaceSwitchStatus::Idle;
            self.workspace_warning = None;
            self.request_notice = Some(success_notice);
        }
        cx.notify();
    }
}
