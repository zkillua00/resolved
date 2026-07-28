use super::*;

impl ApiTester {
    pub(super) fn refresh_variable_intelligence(&mut self, cx: &mut Context<Self>) {
        self.template_highlight_tasks.clear();
        update_script_variable_catalog(&self.script_variable_catalog, &self.workspace);
        self.template_variable_catalog
            .borrow_mut()
            .replace_environment(self.workspace.active_environment());
        self.pre_request_script
            .update(cx, |editor, cx| editor.refresh_diagnostics(cx));
        self.post_response_script
            .update(cx, |editor, cx| editor.refresh_diagnostics(cx));

        let mut inputs =
            Vec::with_capacity(2 + self.headers.len() * 2 + self.body_fields.len() * 2);
        inputs.push(self.url.clone());
        inputs.push(self.body.read(cx).input_state());
        for row in &self.headers {
            inputs.push(row.name.clone());
            inputs.push(row.value.clone());
        }
        for row in &self.body_fields {
            inputs.push(row.name.clone());
            inputs.push(row.value.clone());
        }
        for input in inputs {
            self.refresh_template_input(&input, cx);
        }
    }

    pub(super) fn refresh_template_input(
        &self,
        input: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) {
        let source = input.read(cx).text().to_string();
        let colors = TemplateHighlightColors {
            valid: cx.api_primary_lavender(),
            warning: cx.theme().warning,
            error: cx.theme().red,
        };
        let catalog = self.template_variable_catalog.borrow();
        let semantic_highlights = semantic_style_spans(&source, &catalog, colors);
        drop(catalog);

        input.update(cx, |input, cx| {
            input.set_semantic_highlights(semantic_highlights, cx);
        });
    }

    pub(super) fn schedule_template_input_refresh(
        &mut self,
        input: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) {
        if self.request_dirty.is_hydrating() {
            return;
        }
        let input_id = input.entity_id();
        let input = input.downgrade();
        let task = cx.spawn(async move |this, cx| {
            Timer::after(TEMPLATE_HIGHLIGHT_DEBOUNCE).await;
            let (Some(this), Some(input)) = (this.upgrade(), input.upgrade()) else {
                return;
            };
            this.update(cx, |this, cx| {
                this.refresh_template_input(&input, cx);
            })
            .ok();
        });
        self.template_highlight_tasks.insert(input_id, task);
    }

    pub(super) fn focused_single_line_template_input(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<Entity<InputState>> {
        self.focused_template_input
            .as_ref()
            .filter(|input| input.read(cx).focus_handle(cx).is_focused(window))
            .cloned()
    }

    pub(super) fn track_template_input_focus(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
    ) {
        match event {
            InputEvent::Focus => self.focused_template_input = Some(input.clone()),
            InputEvent::Blur
                if self
                    .focused_template_input
                    .as_ref()
                    .is_some_and(|focused| focused.entity_id() == input.entity_id()) =>
            {
                self.focused_template_input = None;
            }
            _ => {}
        }
    }

    pub(super) fn capture_template_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.is_held {
            return;
        }
        let modifiers = event.keystroke.modifiers;
        if modifiers.platform || modifiers.control || modifiers.function {
            return;
        }
        let Some(typed) = event.keystroke.key_char.as_deref() else {
            return;
        };
        if typed.chars().count() != 1 {
            return;
        }
        if !matches!(typed, "{" | "}") {
            return;
        }
        let Some(input) = self.focused_single_line_template_input(window, cx) else {
            return;
        };
        let handled = input.update(cx, |input, cx| {
            if EntityInputHandler::marked_text_range(input, window, cx).is_some() {
                return false;
            }
            apply_template_pair_edit(input, typed, window, cx)
        });
        if handled {
            cx.stop_propagation();
        }
    }

    pub(super) fn open_template_variable_popover(
        &mut self,
        input: Entity<InputState>,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left {
            return;
        }
        let Some(clicked_utf16) = input.update(cx, |input, cx| {
            EntityInputHandler::character_index_for_point(input, event.position, window, cx)
        }) else {
            self.template_variable_popover = None;
            cx.notify();
            return;
        };
        let source = input.read(cx).text().to_string();
        let clicked_offset = input.read(cx).text().offset_utf16_to_offset(clicked_utf16);
        let Some(span) = scan_template_spans(&source).into_iter().find(|span| {
            span.complete && span.range.start <= clicked_offset && clicked_offset < span.range.end
        }) else {
            self.template_variable_popover = None;
            cx.notify();
            return;
        };
        let name = span.name(&source).to_owned();
        let catalog = self.template_variable_catalog.borrow();
        let action = match span.classification(&source, &catalog) {
            TemplateClassification::Missing => TemplateVariableAction::Create,
            TemplateClassification::Disabled => {
                let Some(variable) = catalog.variable(&name) else {
                    return;
                };
                TemplateVariableAction::Enable {
                    variable_id: variable.id.clone(),
                }
            }
            TemplateClassification::Available | TemplateClassification::Invalid(_) => {
                drop(catalog);
                self.template_variable_popover = None;
                cx.notify();
                return;
            }
        };
        let expected_environment_id = catalog.environment_id().map(ToOwned::to_owned);
        let environment_name = catalog.environment_name().map(ToOwned::to_owned);
        drop(catalog);

        let value = cx.new(|cx| InputState::new(window, cx).placeholder("Variable value"));
        let should_focus_value =
            matches!(action, TemplateVariableAction::Create) && expected_environment_id.is_some();
        self.template_variable_popover = Some(TemplateVariablePopover {
            name,
            expected_environment_id,
            environment_name,
            action,
            value: value.clone(),
            position: event.position,
            error: None,
        });
        if should_focus_value {
            value.read(cx).focus_handle(cx).focus(window);
        }
        cx.notify();
    }

    pub(super) fn template_variable_mutation_blocker(
        &self,
        popover: &TemplateVariablePopover,
        cx: &App,
    ) -> Option<String> {
        let Some(expected_environment_id) = popover.expected_environment_id.as_deref() else {
            return Some("Select an active environment before creating variables.".to_owned());
        };
        if self.workspace.active_environment_id.as_deref() != Some(expected_environment_id) {
            return Some(
                "The active environment changed. Close this popover and try again.".to_owned(),
            );
        }
        if !self.workspace_writable {
            return Some("Environment storage is read-only for this session.".to_owned());
        }
        if self.sending {
            return Some(
                "Wait for the current request to finish before changing variables.".to_owned(),
            );
        }
        if self.active_environment_editor_is_dirty(cx) {
            return Some("Save or revert the active environment's pending edits first.".to_owned());
        }
        None
    }

    pub(super) fn apply_template_variable_action(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(popover) = self.template_variable_popover.clone() else {
            return;
        };
        if self
            .template_variable_mutation_blocker(&popover, cx)
            .is_some()
        {
            if let Some(current) = self.template_variable_popover.as_mut() {
                // Mutation blockers are derived from live application state and
                // rendered directly by the popover. Do not cache one as a
                // persistence error or it can outlive the condition that caused it.
                current.error = None;
            }
            cx.notify();
            return;
        }
        let environment_id = popover
            .expected_environment_id
            .as_deref()
            .expect("mutation blocker requires an active environment");
        let mut candidate = self.workspace.clone();
        let result = match &popover.action {
            TemplateVariableAction::Create => candidate
                .add_environment_variable(
                    environment_id,
                    popover.name.clone(),
                    popover.value.read(cx).value().to_string(),
                    true,
                    false,
                )
                .map(|_| ()),
            TemplateVariableAction::Enable { variable_id } => {
                let variable = candidate
                    .environment(environment_id)
                    .and_then(|environment| {
                        environment
                            .variables
                            .iter()
                            .find(|variable| variable.id == *variable_id)
                    })
                    .cloned();
                variable.map_or_else(
                    || {
                        Err(crate::core::WorkspaceMutationError::NotFound {
                            kind: "variable",
                            id: variable_id.clone(),
                        })
                    },
                    |variable| {
                        candidate.update_environment_variable(
                            environment_id,
                            variable_id,
                            variable.key,
                            variable.value,
                            true,
                            variable.secret,
                        )
                    },
                )
            }
        };
        if let Err(error) = result {
            if let Some(current) = self.template_variable_popover.as_mut() {
                current.error = Some(error.to_string());
            }
            cx.notify();
            return;
        }
        if let Err(error) = self.commit_workspace(candidate) {
            if let Some(current) = self.template_variable_popover.as_mut() {
                current.error = Some(error);
            }
            cx.notify();
            return;
        }

        if self.selected_environment_id.as_deref() == Some(environment_id) {
            self.reload_environment_editor(window, cx);
        }
        self.refresh_variable_intelligence(cx);
        self.request_notice = Some(match popover.action {
            TemplateVariableAction::Create => {
                format!("Created environment variable '{}'.", popover.name)
            }
            TemplateVariableAction::Enable { .. } => {
                format!("Enabled environment variable '{}'.", popover.name)
            }
        });
        self.template_variable_popover = None;
        cx.notify();
    }

    pub(super) fn close_template_variable_popover(&mut self, cx: &mut Context<Self>) {
        self.template_variable_popover = None;
        cx.notify();
    }

    pub(super) fn open_environments_from_template(&mut self, cx: &mut Context<Self>) {
        self.sidebar_tab = SidebarTab::Environments;
        self.template_variable_popover = None;
        cx.notify();
    }
}

pub(super) fn update_script_variable_catalog(
    catalog: &Rc<RefCell<ScriptVariableCatalog>>,
    workspace: &Workspace,
) {
    let enabled_names = workspace
        .active_environment()
        .into_iter()
        .flat_map(|environment| environment.variables.iter())
        .filter(|variable| variable.enabled)
        .map(|variable| variable.key.clone())
        .collect::<Vec<_>>();
    let disabled_names = workspace
        .active_environment()
        .into_iter()
        .flat_map(|environment| environment.variables.iter())
        .filter(|variable| !variable.enabled)
        .map(|variable| variable.key.clone())
        .collect::<Vec<_>>();
    catalog
        .borrow_mut()
        .replace(enabled_names, disabled_names, std::iter::empty::<String>());
}

pub(super) fn template_input_state(
    window: &mut Window,
    cx: &mut Context<InputState>,
    catalog: TemplateVariableCatalogHandle,
    placeholder: impl Into<SharedString>,
    default_value: impl Into<SharedString>,
) -> InputState {
    let hover_catalog = Rc::clone(&catalog);
    let mut state = InputState::new(window, cx)
        .placeholder(placeholder)
        .default_value(default_value);
    state.lsp.completion_provider = Some(Rc::new(TemplateCompletionProvider::new(catalog)));
    state.lsp.hover_provider = Some(Rc::new(TemplateHoverProvider::new(hover_catalog)));
    state
}
