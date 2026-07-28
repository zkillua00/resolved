use super::*;

impl ApiTester {
    pub(super) fn active_environment_editor_is_dirty(&self, cx: &App) -> bool {
        self.workspace.active_environment_id == self.selected_environment_id
            && self.environment_editor_is_dirty(cx)
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
        if !self.workspace_writable {
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
        if !self.workspace_writable || self.sending {
            return;
        }
        let mut candidate = self.workspace.clone();
        match candidate.set_active_environment(id.as_deref()) {
            Ok(()) => {
                if self.commit_workspace(candidate).is_ok() {
                    self.refresh_variable_intelligence(cx);
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(super) fn save_environment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace_writable {
            return;
        }
        let Some(environment_id) = self.selected_environment_id.clone() else {
            return;
        };
        let name = self.environment_name.read(cx).value().to_string();
        let rows = self
            .environment_variables
            .iter()
            .map(|row| {
                (
                    row.id.clone(),
                    row.key.read(cx).value().to_string(),
                    row.value.read(cx).value().to_string(),
                    row.enabled,
                    row.secret,
                )
            })
            .collect::<Vec<_>>();
        let mut candidate = self.workspace.clone();
        let result = (|| {
            candidate.rename_environment(&environment_id, name)?;
            let current = candidate
                .environment(&environment_id)
                .cloned()
                .ok_or_else(|| crate::core::WorkspaceMutationError::NotFound {
                    kind: "environment",
                    id: environment_id.clone(),
                })?;
            let mut replacement = Environment {
                id: current.id,
                name: candidate
                    .environment(&environment_id)
                    .expect("renamed environment must still exist")
                    .name
                    .clone(),
                variables: Vec::with_capacity(rows.len()),
            };
            for (id, key, value, enabled, secret) in &rows {
                replacement.add_variable(key.clone(), value.clone(), *enabled, *secret)?;
                if !id.starts_with("draft-variable-") {
                    replacement
                        .variables
                        .last_mut()
                        .expect("add_variable must append")
                        .id = id.clone();
                }
            }
            let environment = candidate
                .environments
                .iter_mut()
                .find(|environment| environment.id == environment_id)
                .ok_or_else(|| crate::core::WorkspaceMutationError::NotFound {
                    kind: "environment",
                    id: environment_id.clone(),
                })?;
            *environment = replacement;
            Ok::<_, crate::core::WorkspaceMutationError>(())
        })();

        match result {
            Ok(()) => {
                if self.commit_workspace(candidate).is_ok() {
                    self.reload_environment_editor(window, cx);
                    self.refresh_variable_intelligence(cx);
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
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
        if !self.workspace_writable || self.sending {
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
        if !self.workspace_writable {
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
}
