use super::environments_actions::UpstreamEnvironmentMutation;
use super::*;

impl ApiTester {
    pub(super) fn refresh_variable_intelligence(&mut self, cx: &mut Context<Self>) {
        self.dismiss_template_variable_popover();
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
        self.template_variable_hover_task = None;
        let closes_popover = self
            .template_variable_popover
            .as_ref()
            .is_some_and(|popover| popover.source_input_id == input.entity_id());
        if closes_popover && self.dismiss_template_variable_popover() {
            cx.notify();
        }
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
        if event.keystroke.key == "escape" && self.template_variable_popover.is_some() {
            self.close_template_variable_popover(cx);
            cx.stop_propagation();
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
        self.template_variable_source_hovered = Some(input.entity_id());
        self.template_variable_hover_task = None;
        self.show_template_variable_popover(input, event.position, true, window, cx);
    }

    pub(super) fn hover_template_variable_popover(
        &mut self,
        input: Entity<InputState>,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.pressed_button.is_some() {
            return;
        }
        let input_id = input.entity_id();
        self.template_variable_source_hovered = Some(input_id);
        let position = event.position;
        let input = input.downgrade();
        self.template_variable_hover_task = Some(cx.spawn_in(window, async move |this, cx| {
            Timer::after(TEMPLATE_HOVER_DEBOUNCE).await;
            let Some(input) = input.upgrade() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.template_variable_hover_task = None;
                if this.template_variable_source_hovered != Some(input_id) {
                    return;
                }
                this.show_template_variable_popover(input, position, false, window, cx);
            });
        }));
    }

    fn show_template_variable_popover(
        &mut self,
        input: Entity<InputState>,
        position: Point<Pixels>,
        focus_value: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(clicked_utf16) = input.update(cx, |input, cx| {
            EntityInputHandler::character_index_for_point(input, position, window, cx)
        }) else {
            if focus_value || !self.template_variable_popover_should_stay_open(window, cx) {
                self.close_template_variable_popover(cx);
            }
            return;
        };
        let source = input.read(cx).text().to_string();
        let clicked_offset = input.read(cx).text().offset_utf16_to_offset(clicked_utf16);
        let Some(span) = scan_template_spans(&source).into_iter().find(|span| {
            span.complete && span.range.start <= clicked_offset && clicked_offset < span.range.end
        }) else {
            if focus_value || !self.template_variable_popover_should_stay_open(window, cx) {
                self.close_template_variable_popover(cx);
            }
            return;
        };
        let name = span.name(&source).to_owned();
        let catalog = self.template_variable_catalog.borrow();
        let Some(target) = template_variable_popover_target(
            &name,
            span.classification(&source, &catalog),
            &catalog,
        ) else {
            if focus_value || !self.template_variable_popover_should_stay_open(window, cx) {
                drop(catalog);
                self.close_template_variable_popover(cx);
            }
            return;
        };
        let expected_environment_id = catalog.environment_id().map(ToOwned::to_owned);
        let environment_name = catalog.environment_name().map(ToOwned::to_owned);
        drop(catalog);

        let source_input_id = input.entity_id();
        if let Some(current) = self.template_variable_popover.as_ref() {
            let same_target = current.source_input_id == source_input_id
                && current.source_range == span.range
                && current.name == name
                && current.expected_environment_id == expected_environment_id
                && current.action == target.action;
            if same_target {
                if focus_value && expected_environment_id.is_some() {
                    current.value.read(cx).focus_handle(cx).focus(window);
                }
                return;
            }
            if !focus_value && current.value.read(cx).focus_handle(cx).is_focused(window) {
                return;
            }
        }

        let initial_value = target.initial_value;
        let secret = target.secret;
        let value = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Variable value")
                .default_value(initial_value)
                .masked(secret)
        });
        self.template_variable_popover = Some(TemplateVariablePopover {
            source_input: input,
            source_input_id,
            source_range: span.range,
            name,
            expected_environment_id,
            environment_name,
            action: target.action,
            secret,
            value: value.clone(),
            position: position + point(px(12.), px(16.)),
            error: None,
        });
        self.template_variable_popover_hovered = false;
        if focus_value
            && self
                .template_variable_popover
                .as_ref()
                .is_some_and(|popover| popover.expected_environment_id.is_some())
        {
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
            return Some("Select an active environment before editing variables.".to_owned());
        };
        if self.workspace.active_environment_id.as_deref() != Some(expected_environment_id) {
            return Some(
                "The active environment changed. Close this popover and try again.".to_owned(),
            );
        }
        if !self.can_mutate_environment_content() {
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
        let source = popover.source_input.read(cx).text().to_string();
        if !template_variable_source_matches(&source, &popover.source_range, popover.name.as_str())
        {
            if let Some(current) = self.template_variable_popover.as_mut() {
                current.error =
                    Some("The template changed. Hover it again before saving.".to_owned());
            }
            cx.notify();
            return;
        }
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
        let result = apply_template_variable_mutation(
            &mut candidate,
            environment_id,
            &popover.name,
            &popover.action,
            popover.value.read(cx).unmask_value().to_string(),
        );
        if let Err(error) = result {
            if let Some(current) = self.template_variable_popover.as_mut() {
                current.error = Some(error.to_string());
            }
            cx.notify();
            return;
        }
        if !self.workspace_writable {
            let baseline = self
                .workspace
                .environment(environment_id)
                .cloned()
                .expect("mutation blocker requires an active environment");
            let draft = candidate
                .environment(environment_id)
                .cloned()
                .expect("template variable mutation must preserve its environment");
            self.dismiss_template_variable_popover();
            self.mutate_environment_on_upstream(
                UpstreamEnvironmentMutation::Save { baseline, draft },
                window,
                cx,
            );
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
        self.request_notice = Some(match &popover.action {
            TemplateVariableAction::Create => {
                format!("Created environment variable '{}'.", popover.name)
            }
            TemplateVariableAction::Update {
                enable_on_save: true,
                ..
            } => {
                format!(
                    "Updated and enabled environment variable '{}'.",
                    popover.name
                )
            }
            TemplateVariableAction::Update {
                enable_on_save: false,
                ..
            } => {
                format!("Updated environment variable '{}'.", popover.name)
            }
        });
        self.dismiss_template_variable_popover();
        cx.notify();
    }

    pub(super) fn dismiss_template_variable_popover(&mut self) -> bool {
        let changed = self.template_variable_popover.take().is_some();
        self.template_variable_hover_task = None;
        self.template_variable_source_hovered = None;
        self.template_variable_popover_hovered = false;
        changed
    }

    fn template_variable_popover_should_stay_open(&self, window: &Window, cx: &App) -> bool {
        self.template_variable_popover_hovered
            || self
                .template_variable_popover
                .as_ref()
                .is_some_and(|popover| popover.value.read(cx).focus_handle(cx).is_focused(window))
    }

    pub(super) fn close_template_variable_popover(&mut self, cx: &mut Context<Self>) {
        if self.dismiss_template_variable_popover() {
            cx.notify();
        }
    }

    pub(super) fn set_template_variable_popover_hovered(
        &mut self,
        hovered: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.template_variable_popover_hovered = hovered;
        self.template_variable_hover_task = None;
        if self.template_variable_popover_should_stay_open(window, cx)
            || self.template_variable_popover.is_none()
        {
            return;
        }
        self.schedule_template_variable_popover_dismissal(window, cx);
    }

    pub(super) fn set_template_variable_source_hovered(
        &mut self,
        input_id: EntityId,
        hovered: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if hovered {
            self.template_variable_source_hovered = Some(input_id);
            self.template_variable_hover_task = None;
            return;
        }
        if self.template_variable_source_hovered != Some(input_id) {
            return;
        }
        self.template_variable_source_hovered = None;
        self.template_variable_hover_task = None;
        let owns_popover = self
            .template_variable_popover
            .as_ref()
            .is_some_and(|popover| popover.source_input_id == input_id);
        if owns_popover && !self.template_variable_popover_should_stay_open(window, cx) {
            self.schedule_template_variable_popover_dismissal(window, cx);
        }
    }

    fn schedule_template_variable_popover_dismissal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.template_variable_hover_task = Some(cx.spawn_in(window, async move |this, cx| {
            Timer::after(TEMPLATE_HOVER_DEBOUNCE).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.template_variable_hover_task = None;
                if !this.template_variable_popover_should_stay_open(window, cx)
                    && this.dismiss_template_variable_popover()
                {
                    cx.notify();
                }
            });
        }));
    }

    pub(super) fn open_environments_from_template(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_template_variable_popover();
        self.activate_request_workspace(SidebarTab::Environments, window, cx);
    }
}

fn template_variable_source_matches(
    source: &str,
    expected_range: &std::ops::Range<usize>,
    expected_name: &str,
) -> bool {
    scan_template_spans(source).into_iter().any(|span| {
        span.complete
            && span.range == *expected_range
            && span.name(source).trim() == expected_name.trim()
    })
}

pub(super) fn update_script_variable_catalog(
    catalog: &Rc<RefCell<ScriptVariableCatalog>>,
    workspace: &Workspace,
) {
    let enabled_variables = workspace
        .active_environment()
        .into_iter()
        .flat_map(|environment| environment.variables.iter())
        .filter(|variable| variable.enabled)
        .map(|variable| (variable.key.clone(), variable.value.clone()))
        .collect::<Vec<_>>();
    let disabled_names = workspace
        .active_environment()
        .into_iter()
        .flat_map(|environment| environment.variables.iter())
        .filter(|variable| !variable.enabled)
        .map(|variable| variable.key.clone())
        .collect::<Vec<_>>();
    catalog.borrow_mut().replace_with_environment_values(
        enabled_variables,
        disabled_names,
        std::iter::empty::<String>(),
    );
}

pub(super) fn template_input_state(
    window: &mut Window,
    cx: &mut Context<InputState>,
    catalog: TemplateVariableCatalogHandle,
    placeholder: impl Into<SharedString>,
    default_value: impl Into<SharedString>,
) -> InputState {
    let mut state = InputState::new(window, cx)
        .placeholder(placeholder)
        .default_value(default_value);
    state.lsp.completion_provider = Some(Rc::new(TemplateCompletionProvider::new(catalog)));
    state
}

#[derive(Debug, PartialEq, Eq)]
struct TemplateVariablePopoverTarget {
    action: TemplateVariableAction,
    initial_value: String,
    secret: bool,
}

fn template_variable_popover_target(
    name: &str,
    classification: TemplateClassification,
    catalog: &TemplateVariableCatalog,
) -> Option<TemplateVariablePopoverTarget> {
    match classification {
        TemplateClassification::Missing => Some(TemplateVariablePopoverTarget {
            action: TemplateVariableAction::Create,
            initial_value: String::new(),
            secret: false,
        }),
        TemplateClassification::Available | TemplateClassification::Disabled => {
            let variable = catalog.variable(name)?;
            Some(TemplateVariablePopoverTarget {
                action: TemplateVariableAction::Update {
                    variable_id: variable.id.clone(),
                    enable_on_save: classification == TemplateClassification::Disabled,
                },
                initial_value: variable.value.clone(),
                secret: variable.secret,
            })
        }
        TemplateClassification::Invalid(_) => None,
    }
}

fn apply_template_variable_mutation(
    workspace: &mut Workspace,
    environment_id: &str,
    name: &str,
    action: &TemplateVariableAction,
    value: String,
) -> Result<(), crate::core::WorkspaceMutationError> {
    match action {
        TemplateVariableAction::Create => workspace
            .add_environment_variable(environment_id, name, value, true, false)
            .map(|_| ()),
        TemplateVariableAction::Update {
            variable_id,
            enable_on_save,
        } => {
            let variable = workspace
                .environment(environment_id)
                .and_then(|environment| environment.variable(variable_id))
                .cloned()
                .ok_or_else(|| crate::core::WorkspaceMutationError::NotFound {
                    kind: "variable",
                    id: variable_id.clone(),
                })?;
            workspace.update_environment_variable(
                environment_id,
                variable_id,
                variable.key,
                value,
                variable.enabled || *enable_on_save,
                variable.secret,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template_intelligence::TemplateVariable;

    fn variable_completion_documentation(
        provider: &ScriptCompletionProvider,
        source: &str,
    ) -> String {
        let item = provider
            .completion_items_for_source(source, source.len())
            .into_iter()
            .next()
            .expect("expected environment-variable completion");
        match item.documentation {
            Some(lsp_types::Documentation::String(documentation)) => documentation,
            Some(lsp_types::Documentation::MarkupContent(markup)) => markup.value,
            None => panic!("expected environment-variable documentation"),
        }
    }

    fn catalog() -> TemplateVariableCatalog {
        TemplateVariableCatalog::from_parts(
            Some("env-1".to_owned()),
            Some("Development".to_owned()),
            [
                TemplateVariable {
                    id: "enabled-id".to_owned(),
                    name: "base_url".to_owned(),
                    value: "https://example.test".to_owned(),
                    enabled: true,
                    secret: false,
                },
                TemplateVariable {
                    id: "disabled-id".to_owned(),
                    name: "token".to_owned(),
                    value: "top-secret".to_owned(),
                    enabled: false,
                    secret: true,
                },
            ],
        )
    }

    #[test]
    fn popover_targets_preload_existing_values_and_prepare_missing_values() {
        let catalog = catalog();

        let enabled = template_variable_popover_target(
            "base_url",
            TemplateClassification::Available,
            &catalog,
        )
        .unwrap();
        assert_eq!(enabled.initial_value, "https://example.test");
        assert!(!enabled.secret);
        assert_eq!(
            enabled.action,
            TemplateVariableAction::Update {
                variable_id: "enabled-id".to_owned(),
                enable_on_save: false,
            }
        );

        let disabled =
            template_variable_popover_target("token", TemplateClassification::Disabled, &catalog)
                .unwrap();
        assert_eq!(disabled.initial_value, "top-secret");
        assert!(disabled.secret);
        assert_eq!(
            disabled.action,
            TemplateVariableAction::Update {
                variable_id: "disabled-id".to_owned(),
                enable_on_save: true,
            }
        );

        let missing = template_variable_popover_target(
            "new_value",
            TemplateClassification::Missing,
            &catalog,
        )
        .unwrap();
        assert_eq!(missing.initial_value, "");
        assert_eq!(missing.action, TemplateVariableAction::Create);
    }

    #[test]
    fn workspace_refreshes_values_for_script_editors_but_not_plain_snippets() {
        let mut workspace = Workspace::default();
        let environment_id = workspace.create_environment("Development").unwrap();
        let variable_id = workspace
            .add_environment_variable(&environment_id, "api_token", "first-secret", true, true)
            .unwrap();
        workspace
            .add_environment_variable(
                &environment_id,
                "disabled_token",
                "disabled-secret",
                false,
                true,
            )
            .unwrap();
        workspace
            .set_active_environment(Some(&environment_id))
            .unwrap();

        let catalog = ScriptVariableCatalog::default().shared();
        update_script_variable_catalog(&catalog, &workspace);
        let script =
            ScriptCompletionProvider::new(ScriptEditorPhase::PreRequest, Rc::clone(&catalog));
        let plain = ScriptCompletionProvider::for_plain_snippet(
            ScriptEditorPhase::PreRequest,
            Rc::clone(&catalog),
        );
        let source = r#"api.environment.get("api"#;

        assert!(variable_completion_documentation(&script, source).contains("first-secret"));
        assert!(!variable_completion_documentation(&plain, source).contains("first-secret"));

        workspace
            .update_environment_variable(
                &environment_id,
                &variable_id,
                "api_token",
                "second-secret",
                true,
                true,
            )
            .unwrap();
        update_script_variable_catalog(&catalog, &workspace);

        let script_documentation = variable_completion_documentation(&script, source);
        assert!(script_documentation.contains("second-secret"));
        assert!(!script_documentation.contains("first-secret"));
        let plain_documentation = variable_completion_documentation(&plain, source);
        assert!(!plain_documentation.contains("second-secret"));
        assert!(!plain_documentation.contains("disabled-secret"));
    }

    #[test]
    fn source_revalidation_rejects_an_equal_length_template_rename() {
        assert!(template_variable_source_matches(
            "{{foo}}",
            &(0.."{{foo}}".len()),
            "foo"
        ));
        assert!(!template_variable_source_matches(
            "{{bar}}",
            &(0.."{{bar}}".len()),
            "foo"
        ));
    }

    #[test]
    fn variable_mutations_create_update_and_enable_without_losing_secret_status() {
        let mut workspace = Workspace::default();
        let environment_id = workspace.create_environment("Development").unwrap();
        let enabled_id = workspace
            .add_environment_variable(&environment_id, "base_url", "https://old.test", true, false)
            .unwrap();
        let disabled_id = workspace
            .add_environment_variable(&environment_id, "token", "old-secret", false, true)
            .unwrap();

        apply_template_variable_mutation(
            &mut workspace,
            &environment_id,
            "base_url",
            &TemplateVariableAction::Update {
                variable_id: enabled_id.clone(),
                enable_on_save: false,
            },
            "https://new.test".to_owned(),
        )
        .unwrap();
        apply_template_variable_mutation(
            &mut workspace,
            &environment_id,
            "token",
            &TemplateVariableAction::Update {
                variable_id: disabled_id.clone(),
                enable_on_save: true,
            },
            "new-secret".to_owned(),
        )
        .unwrap();
        apply_template_variable_mutation(
            &mut workspace,
            &environment_id,
            "region",
            &TemplateVariableAction::Create,
            "eu-west".to_owned(),
        )
        .unwrap();

        let environment = workspace.environment(&environment_id).unwrap();
        let enabled = environment.variable(&enabled_id).unwrap();
        assert_eq!(enabled.value, "https://new.test");
        assert!(enabled.enabled);
        assert!(!enabled.secret);
        let disabled = environment.variable(&disabled_id).unwrap();
        assert_eq!(disabled.value, "new-secret");
        assert!(disabled.enabled);
        assert!(disabled.secret);
        let created = environment
            .variables
            .iter()
            .find(|variable| variable.key == "region")
            .unwrap();
        assert_eq!(created.value, "eu-west");
        assert!(created.enabled);
        assert!(!created.secret);
    }
}
