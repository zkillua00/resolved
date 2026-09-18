use super::*;
use crate::core::execution_limits::{Bound, DEFAULT_LIMITS, ExecutionLimits};
use crate::core::local_execution_limits::{Overrides, local_scope_path, validate_overrides};
use gpui_component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_component::tab::Tab;
use std::collections::BTreeMap;

const LIMIT_SECTIONS: &[(&str, &str, &str)] = &[
    (
        "HTTP",
        "http.",
        "Request timeouts and sizes. Connect timeout includes TLS; separate TLS timers and relay envelopes only apply to server execution.",
    ),
    (
        "WebSocket",
        "websocket.",
        "Connection, message and automation budgets. Opening relay envelopes only apply to server execution.",
    ),
    (
        "Scripts",
        "script.",
        "Runtime, memory and output budgets for local scripts.",
    ),
    (
        "Chains",
        "chain.",
        "Depth and request budgets for chained request execution.",
    ),
];

fn is_local_limit(key: &str) -> bool {
    !matches!(
        key,
        "http.envelope_bytes" | "websocket.opening_bytes" | "http.tls_handshake_timeout_ms"
    )
}

fn local_limit_label(key: &str) -> &str {
    match key {
        "http.timeout_ms" => "Request timeout",
        "http.connect_timeout_ms" => "Connect timeout",
        "http.request_bytes" => "Request size",
        "http.response_bytes" => "Response size",
        "http.redirects" => "Redirects",
        "http.header_count" => "Header count",
        "http.url_bytes" => "URL size",
        "websocket.handshake_timeout_ms" => "Handshake timeout",
        "websocket.message_bytes" => "Message size",
        "websocket.concurrent_sessions" => "Concurrent sessions",
        "websocket.script_source_bytes" => "Automation source size",
        "websocket.script_modules" => "Automation modules",
        "script.timeout_ms" => "Script timeout",
        "script.memory_bytes" => "Memory",
        "script.stack_bytes" => "Stack size",
        "script.source_bytes" => "Source size",
        "script.body_bytes" => "Body size",
        "script.result_bytes" => "Result size",
        "script.log_entries" => "Log entries",
        "script.log_bytes" => "Log size",
        "chain.max_depth" => "Maximum depth",
        "chain.max_requests" => "Maximum requests",
        _ => key,
    }
}

fn local_limit_unit(key: &str) -> &'static str {
    if key.ends_with("_ms") {
        "ms"
    } else if key.ends_with("_bytes") {
        "bytes"
    } else {
        "count"
    }
}

fn saved_limit_label(key: &str, bound: Bound) -> String {
    if bound.unlimited {
        return "Unlimited".into();
    }
    let unit = local_limit_unit(key);
    let exact = format!("{} {unit}", bound.value);
    match unit {
        "ms" if bound.value >= 1_000 => {
            format!(
                "{exact} ({})",
                format_duration(Duration::from_millis(bound.value as u64))
            )
        }
        "bytes" if bound.value >= 1_024 => {
            format!("{exact} ({})", format_bytes(bound.value as usize))
        }
        "count" => bound.value.to_string(),
        _ => exact,
    }
}

fn parse_limit_override(text: &str) -> Result<Option<Bound>, String> {
    let text = text.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("inherit") {
        return Ok(None);
    }
    if text.eq_ignore_ascii_case("unlimited") {
        return Ok(Some(Bound::unlimited()));
    }
    let value = text
        .parse::<i64>()
        .ok()
        .filter(|value| *value >= 0)
        .ok_or("Enter a non-negative integer, Inherit or Unlimited")?;
    Ok(Some(Bound::limited(value)))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Scope {
    workspace: Option<String>,
    collection: Option<String>,
    folder: Option<String>,
}

pub(super) struct LocalExecutionLimitEditor {
    scope: Scope,
    inputs: BTreeMap<String, Entity<InputState>>,
    section: usize,
    scroll_handle: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl ApiTester {
    pub(super) fn local_execution_limits_for_request(
        &self,
        request_id: Option<&str>,
    ) -> Result<ExecutionLimits, String> {
        let WorkspaceProviderId::Local(workspace_id) = self.workspace_providers.active_id() else {
            return Err("Local execution limits require a local workspace".into());
        };
        let mut collection_id = None;
        let mut folder_id = None;
        if let Some(request_id) = request_id {
            let mut matches = self.workspace.collections.iter().flat_map(|collection| {
                collection
                    .requests
                    .iter()
                    .filter(move |request| request.id == request_id)
                    .map(move |request| (collection.id.as_str(), request.folder_id.as_deref()))
            });
            let (collection, folder) = matches
                .next()
                .ok_or("Request not found in the local workspace")?;
            if matches.next().is_some() {
                return Err("Ambiguous local request identity".into());
            }
            collection_id = Some(collection);
            folder_id = folder;
        }
        self.settings.execution_limits.resolve_for_request(
            workspace_id,
            &self.workspace,
            collection_id,
            folder_id,
            self.settings.script.timeout(),
        )
    }

    pub(super) fn local_execution_limits_settings_page(
        &self,
        cx: &mut Context<Self>,
    ) -> SettingPage {
        let this = cx.entity().downgrade();
        SettingPage::new("Execution limits")
            .description("Local HTTP, WebSocket, script and chain budgets. Server workspaces use Proxy → Execution limits.")
            .resettable(false)
            .full_bleed()
            .group(SettingGroup::new().item(SettingItem::render_searchable(
                "Local policies execution limits HTTP WebSocket scripts chains scope overrides inherit unlimited",
                move |_, window, cx| {
                    let Some(this) = this.upgrade() else { return div().into_any_element(); };
                    this.update(cx, |this, cx| this.render_local_execution_limits(window, cx))
                },
            )))
    }

    fn local_limit_overrides(&self, scope: &Scope) -> Overrides {
        let settings = &self.settings.execution_limits;
        match (
            &scope.workspace,
            scope.folder.as_ref().or(scope.collection.as_ref()),
        ) {
            (None, _) => settings.application.clone(),
            (Some(workspace), None) => settings
                .workspaces
                .get(workspace)
                .cloned()
                .unwrap_or_default(),
            (Some(workspace), Some(resource)) => settings
                .collections
                .get(workspace)
                .and_then(|c| c.get(resource))
                .cloned()
                .unwrap_or_default(),
        }
    }

    fn local_limit_scope_dirty(&self, cx: &App) -> bool {
        let Some(editor) = &self.local_execution_limit_editor else {
            return false;
        };
        let saved = self.local_limit_overrides(&editor.scope);
        editor.inputs.iter().any(|(key, input)| {
            match parse_limit_override(input.read(cx).value().as_ref()) {
                Ok(value) => saved.get(key).copied() != value,
                Err(_) => true,
            }
        })
    }

    fn select_local_limit_scope(
        &mut self,
        scope: Scope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let overrides = self.local_limit_overrides(&scope);
        let section = self
            .local_execution_limit_editor
            .as_ref()
            .map_or(0, |editor| editor.section);
        let mut subscriptions = Vec::new();
        let inputs = DEFAULT_LIMITS
            .iter()
            .map(|(key, _)| {
                let value = overrides
                    .get(*key)
                    .map(|b| {
                        if b.unlimited {
                            "Unlimited".into()
                        } else {
                            b.value.to_string()
                        }
                    })
                    .unwrap_or_default();
                let input = cx.new(|cx| InputState::new(window, cx).placeholder("Inherit"));
                input.update(cx, |input, cx| input.set_value(value, window, cx));
                subscriptions.push(cx.subscribe(&input, |_, _, event, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                }));
                (key.to_string(), input)
            })
            .collect();
        self.local_execution_limit_editor = Some(LocalExecutionLimitEditor {
            scope,
            inputs,
            section,
            scroll_handle: ScrollHandle::new(),
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    fn save_local_limit_scope(&mut self, reset: bool, window: &mut Window, cx: &mut Context<Self>) {
        let result = (|| {
            let editor = self
                .local_execution_limit_editor
                .as_ref()
                .ok_or("Select a scope first")?;
            let scope = editor.scope.clone();
            if let Some(workspace_id) = &scope.workspace {
                if !self.local_workspaces.iter().any(|w| &w.id == workspace_id) {
                    return Err("Local workspace no longer exists".into());
                }
                if scope.collection.is_some() {
                    if self.workspace_providers.active_id()
                        != &WorkspaceProviderId::Local(workspace_id.clone())
                    {
                        return Err(
                            "Reopen this local workspace before editing its collections".into()
                        );
                    }
                    local_scope_path(
                        &self.workspace,
                        scope.collection.as_deref(),
                        scope.folder.as_deref(),
                    )?;
                }
            }
            let mut values = Overrides::new();
            if !reset {
                for (key, input) in &editor.inputs {
                    if let Some(bound) = parse_limit_override(input.read(cx).value().as_ref())
                        .map_err(|error| format!("{key}: {error}"))?
                    {
                        values.insert(key.clone(), bound);
                    }
                }
            }
            validate_overrides(&values)?;
            let mut settings = self.settings.clone();
            match (
                &scope.workspace,
                scope.folder.as_ref().or(scope.collection.as_ref()),
            ) {
                (None, _) => settings.execution_limits.application = values,
                (Some(workspace), None) => {
                    if values.is_empty() {
                        settings.execution_limits.workspaces.remove(workspace);
                    } else {
                        settings
                            .execution_limits
                            .workspaces
                            .insert(workspace.clone(), values);
                    }
                }
                (Some(workspace), Some(resource)) => {
                    let scopes = settings
                        .execution_limits
                        .collections
                        .entry(workspace.clone())
                        .or_default();
                    if values.is_empty() {
                        scopes.remove(resource);
                    } else {
                        scopes.insert(resource.clone(), values);
                    }
                    if scopes.is_empty() {
                        settings.execution_limits.collections.remove(workspace);
                    }
                }
            }
            self.commit_settings(settings, false, cx)?;
            Ok::<_, String>(())
        })();
        self.settings_notice = Some(match result {
            Ok(()) => {
                if reset {
                    let scope = self
                        .local_execution_limit_editor
                        .as_ref()
                        .unwrap()
                        .scope
                        .clone();
                    self.select_local_limit_scope(scope, window, cx);
                }
                "Local execution limits saved. New executions use these values immediately.".into()
            }
            Err(error) => error,
        });
        cx.notify();
    }

    fn render_local_limit_row(
        &self,
        index: usize,
        key: &'static str,
        effective: Bound,
        source: &str,
        cx: &App,
    ) -> AnyElement {
        let input = self.local_execution_limit_editor.as_ref().unwrap().inputs[key].clone();
        let inherit = input.clone();
        let unlimited = input.clone();
        let draft = parse_limit_override(input.read(cx).value().as_ref());
        let controls = h_flex()
            .debug_selector(|| format!("local-limit-controls-{key}"))
            .gap_1()
            .child(
                div()
                    .debug_selector(|| format!("local-limit-input-{key}"))
                    .w(rems(12.))
                    .child(
                        Input::new(&input)
                            .small()
                            .suffix(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(local_limit_unit(key)),
                            )
                            .disabled(!self.settings_writable),
                    ),
            )
            .child(
                Button::new(("local-limit-inherit", index))
                    .label("Inherit")
                    .small()
                    .ghost()
                    .selected(draft == Ok(None))
                    .disabled(!self.settings_writable)
                    .on_click(move |_, window, cx| {
                        inherit.update(cx, |input, cx| input.set_value("", window, cx))
                    }),
            )
            .child(
                Button::new(("local-limit-unlimited", index))
                    .label("Unlimited")
                    .small()
                    .ghost()
                    .selected(draft == Ok(Some(Bound::unlimited())))
                    .disabled(!self.settings_writable)
                    .on_click(move |_, window, cx| {
                        unlimited.update(cx, |input, cx| input.set_value("Unlimited", window, cx))
                    }),
            );
        v_flex()
            .debug_selector(|| format!("local-limit-row-{key}"))
            .flex_shrink_0()
            .min_w_0()
            .gap_2()
            .py_4()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(rems(14.))
                            .gap_1()
                            .child(div().text_sm().font_medium().child(local_limit_label(key)))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(key),
                            ),
                    )
                    .child(controls),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "Saved effective: {} · From {source}",
                        saved_limit_label(key, effective),
                    )),
            )
            .into_any_element()
    }

    fn render_local_execution_limits(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.local_execution_limit_editor.is_none() {
            self.select_local_limit_scope(Scope::default(), window, cx);
        }
        let editor = self.local_execution_limit_editor.as_ref().unwrap();
        let selected = editor.scope.clone();
        let section = editor.section;
        let scroll_handle = editor.scroll_handle.clone();
        let dirty = self.local_limit_scope_dirty(cx);
        let mut scopes = vec![(
            "Application — all local workspaces".to_owned(),
            Scope::default(),
        )];
        for workspace in &self.local_workspaces {
            scopes.push((
                format!("Workspace: {}", workspace.name),
                Scope {
                    workspace: Some(workspace.id.clone()),
                    ..Default::default()
                },
            ));
        }
        if let WorkspaceProviderId::Local(workspace_id) = self.workspace_providers.active_id() {
            let workspace_name = self
                .local_workspaces
                .iter()
                .find(|workspace| &workspace.id == workspace_id)
                .map(|workspace| workspace.name.as_str())
                .unwrap_or(workspace_id);
            for collection in &self.workspace.collections {
                let scope = Scope {
                    workspace: Some(workspace_id.clone()),
                    collection: Some(collection.id.clone()),
                    folder: None,
                };
                scopes.push((
                    format!("{workspace_name} / {}", collection.name),
                    scope.clone(),
                ));
                for folder in &collection.folders {
                    let name = collection
                        .folder_path_ids(&folder.id)
                        .map(|ids| {
                            ids.iter()
                                .filter_map(|id| collection.folder(id).map(|f| f.name.clone()))
                                .collect::<Vec<_>>()
                                .join(" / ")
                        })
                        .unwrap_or_else(|_| folder.name.clone());
                    scopes.push((
                        format!("{workspace_name} / {} / {name}", collection.name),
                        Scope {
                            folder: Some(folder.id.clone()),
                            ..scope.clone()
                        },
                    ));
                }
            }
        }
        let scope_label = scopes
            .iter()
            .find(|(_, scope)| scope == &selected)
            .map(|(label, _)| label.clone())
            .unwrap_or_else(|| "Scope no longer available — select another scope".into());
        let menu_this = cx.entity().downgrade();
        let menu_selected = selected.clone();
        let selector = h_flex()
            .min_w_0()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Scope"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .font_semibold()
                    .truncate()
                    .child(scope_label.clone()),
            )
            .child(
                Button::new("local-limit-scope")
                    .debug_selector(|| "local-limit-scope-action".to_owned())
                    .label("Change scope")
                    .outline()
                    .dropdown_caret(true)
                    .disabled(dirty)
                    .tooltip(if dirty {
                        "Save or discard your edits before switching scope.".to_owned()
                    } else {
                        scope_label
                    })
                    .dropdown_menu(move |mut menu, _, _| {
                        menu = menu.scrollable(true);
                        for (label, scope) in &scopes {
                            let scope = scope.clone();
                            let this = menu_this.clone();
                            let is_selected = scope == menu_selected;
                            menu = menu.item(
                                PopupMenuItem::new(label.clone())
                                    .checked(is_selected)
                                    .on_click(move |_, window, cx| {
                                        if !is_selected {
                                            if let Some(this) = this.upgrade() {
                                                this.update(cx, |this, cx| {
                                                    this.select_local_limit_scope(
                                                        scope.clone(),
                                                        window,
                                                        cx,
                                                    );
                                                });
                                            }
                                        }
                                    }),
                            );
                        }
                        menu
                    }),
            );
        let mut policy = self.settings.execution_limits.clone();
        if selected.workspace.is_none() {
            policy.workspaces.clear();
            policy.collections.clear();
        }
        let effective = policy.resolve_with_sources(
            selected.workspace.as_deref().unwrap_or(""),
            &self.workspace,
            selected.collection.as_deref(),
            selected.folder.as_deref(),
            self.settings.script.timeout(),
        );
        // Only the fields scroll. The full-bleed Settings item supplies a bounded
        // height; every flex boundary must allow shrinking down to that height.
        let mut fields = v_flex()
            .id("local-limit-fields")
            .debug_selector(|| "local-limit-fields".to_owned())
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_y_scroll()
            .track_scroll(&scroll_handle)
            .px_4()
            .child(
                v_flex()
                    .flex_shrink_0()
                    .gap_2()
                    .py_4()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(LIMIT_SECTIONS[section].2)
                    .child("Blank / Inherit uses the nearest ancestor. Zero is an explicit limit; Unlimited removes the cap. Effective values below reflect saved settings, not your draft.")
                    .child("Collections and folders can be selected for the open local workspace."),
            );
        match effective {
            Err(error) => fields = fields.child(div().text_color(cx.theme().danger).child(error)),
            Ok((effective, sources)) => {
                for (index, (key, _)) in DEFAULT_LIMITS.iter().enumerate() {
                    if !is_local_limit(key) || !key.starts_with(LIMIT_SECTIONS[section].1) {
                        continue;
                    }
                    let source = sources
                        .get(*key)
                        .map(String::as_str)
                        .unwrap_or("Built-in default");
                    fields = fields.child(self.render_local_limit_row(
                        index,
                        key,
                        effective.get(key),
                        source,
                        cx,
                    ));
                }
            }
        }
        v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .child(
                v_flex().flex_shrink_0().gap_3().px_4().pt_4()
                    .child(selector)
                    .child(
                        TabBar::new("local-limit-sections")
                            .underline()
                            .children(LIMIT_SECTIONS.iter().map(|(title, _, _)| {
                                Tab::new()
                                    .label(*title)
                                    .debug_selector(|| format!("local-limit-tab-{title}"))
                            }))
                            .selected_index(section)
                            .on_click(cx.listener(|this, index: &usize, _, cx| {
                                if let Some(editor) = &mut this.local_execution_limit_editor {
                                    editor.section = *index;
                                    editor.scroll_handle.set_offset(point(px(0.), px(0.)));
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .child(fields.vertical_scrollbar(&scroll_handle))
            .child(
                v_flex()
                    .debug_selector(|| "local-limit-actions".to_owned())
                    .flex_shrink_0()
                    .gap_2()
                    .p_4()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div().text_xs().text_color(cx.theme().muted_foreground).child(
                            if !self.settings_writable {
                                "Read only — settings cannot be saved."
                            } else if dirty {
                                "Unsaved changes — save or discard before switching scope."
                            } else {
                                "Saved limits apply to new executions. More specific scopes take precedence."
                            },
                        ),
                    )
                    .child(
                        h_flex().gap_2()
                            .child(
                                div()
                                    .debug_selector(|| "local-limit-reset-action".to_owned())
                                    .child(
                                        Button::new("local-limit-reset")
                                            .label("Reset scope")
                                            .outline()
                                            .tooltip("Immediately remove overrides in all sections of this scope, discard unsaved edits, and inherit from its ancestors.")
                                            .disabled(!self.settings_writable)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.save_local_limit_scope(true, window, cx)
                                            })),
                                    ),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new("local-limit-discard")
                                    .debug_selector(|| "local-limit-discard-action".to_owned())
                                    .label("Discard edits")
                                    .ghost()
                                    .disabled(!dirty)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if let Some(editor) = &this.local_execution_limit_editor {
                                            this.select_local_limit_scope(editor.scope.clone(), window, cx);
                                            this.settings_notice = None;
                                        }
                                    })),
                            )
                            .child(
                                div()
                                    .debug_selector(|| "local-limit-save-action".to_owned())
                                    .child(
                                        Button::new("local-limit-save")
                                            .label("Save changes")
                                            .primary()
                                            .disabled(!self.settings_writable || !dirty)
                                            .tooltip("Save every section in this scope.")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.save_local_limit_scope(false, window, cx)
                                            })),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Bounds, Modifiers, ScrollDelta, TestAppContext, size};
    use gpui_component::{group_box::GroupBoxVariant, setting::Settings};

    #[test]
    fn every_local_limit_has_a_label_and_one_section() {
        for (key, _) in DEFAULT_LIMITS {
            if is_local_limit(key) {
                assert_ne!(local_limit_label(key), *key, "missing label for {key}");
                assert_eq!(
                    LIMIT_SECTIONS
                        .iter()
                        .filter(|(_, prefix, _)| key.starts_with(prefix))
                        .count(),
                    1,
                    "{key} must appear in exactly one section"
                );
            }
        }
    }

    #[test]
    fn override_input_preserves_zero_inherit_and_unlimited() {
        assert_eq!(parse_limit_override(" 0 "), Ok(Some(Bound::limited(0))));
        assert_eq!(parse_limit_override(""), Ok(None));
        assert_eq!(parse_limit_override(" INHERIT "), Ok(None));
        assert_eq!(
            parse_limit_override("unlimited"),
            Ok(Some(Bound::unlimited()))
        );
        for invalid in ["-1", "1.5", "60s", "9223372036854775808"] {
            assert!(parse_limit_override(invalid).is_err());
        }
        assert_eq!(
            saved_limit_label("http.timeout_ms", Bound::limited(60_000)),
            "60000 ms (60.00 s)"
        );
        assert_eq!(
            saved_limit_label("http.response_bytes", Bound::limited(67_108_864)),
            "67108864 bytes (64.0 MiB)"
        );
        assert_eq!(saved_limit_label("http.redirects", Bound::limited(0)), "0");
        assert_eq!(
            saved_limit_label("script.timeout_ms", Bound::unlimited()),
            "Unlimited"
        );
    }

    struct LimitsView(Entity<ApiTester>);
    impl Render for LimitsView {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.0.update(cx, |app, cx| {
                // Exercise the real Settings -> full-bleed page -> custom item
                // chain, including the app title and settings notice overlay.
                let (inset, overlay) = super::super::settings_page::settings_message_overlay(
                    app.settings_warning.clone(),
                    app.settings_notice.clone(),
                    cx,
                );
                v_flex()
                    .size_full()
                    .child(app.render_settings_title_bar(window, cx))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .child(
                                Settings::new("local-limit-settings-test")
                                    .sidebar_width(
                                        super::super::settings_page::SETTINGS_SIDEBAR_WIDTH
                                            .to_pixels(cx.theme().font_size),
                                    )
                                    .content_top_inset(inset)
                                    .with_group_variant(GroupBoxVariant::Outline)
                                    .page(app.local_execution_limits_settings_page(cx)),
                            )
                            .when_some(overlay, |this, overlay| this.child(overlay)),
                    )
            })
        }
    }

    #[gpui::test]
    fn local_limit_settings_keep_actions_visible_and_save_real_policy(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let store = DatabaseStore::new(directory.path().join("limits.sqlite3"));
        store.initialize().unwrap();
        let reload_store = store.clone();
        let mut handles = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let bindings = shortcuts::capture_base_key_bindings(cx);
            let app = cx.new(|cx| ApiTester::new_with_database_store(bindings, store, window, cx));
            let view = cx.new(|_| LimitsView(app.clone()));
            handles = Some((app, view.clone()));
            Root::new(view, window, cx)
        });
        let (app, view) = handles.unwrap();
        cx.update(|window, _| window.activate_window());
        // Normal and compact settings windows must not need a 2600px-tall
        // viewport to expose Save. Zoom must obey the same bounded layout.
        for (width, height, font_size) in [(1100., 720., 14.), (900., 540., 14.), (800., 500., 18.)]
        {
            cx.update(|window, cx| {
                gpui_component::Theme::global_mut(cx).font_size = px(font_size);
                window.refresh();
            });
            cx.simulate_resize(size(px(width), px(height)));
            cx.run_until_parked();
            let viewport = Bounds::new(point(px(0.), px(0.)), size(px(width), px(height)));
            let actions = cx
                .debug_bounds("local-limit-actions")
                .expect("fixed action bar");
            let fields = cx
                .debug_bounds("local-limit-fields")
                .expect("scrolling fields");
            for selector in ["local-limit-save-action", "local-limit-reset-action"] {
                let bounds = cx.debug_bounds(selector).expect("visible action");
                assert!(viewport.contains(&bounds.origin), "{selector}: {bounds:?}");
                assert!(
                    viewport.contains(&bounds.bottom_right()),
                    "{selector}: {bounds:?}"
                );
                assert!(actions.contains(&bounds.center()));
            }
            assert!(fields.size.height > px(0.));
            assert!(
                fields.bottom() <= actions.top() + px(1.),
                "{width}x{height}: fields {fields:?} overlap actions {actions:?}"
            );
            let first = cx.debug_bounds("local-limit-row-http.timeout_ms").unwrap();
            let last = cx.debug_bounds("local-limit-row-http.url_bytes").unwrap();
            let controls = cx
                .debug_bounds("local-limit-controls-http.timeout_ms")
                .unwrap();
            assert!(
                controls.left() >= fields.left() && controls.right() <= fields.right(),
                "field controls must fit horizontally: {controls:?}, {fields:?}"
            );
            assert!(
                first.origin.x < fields.origin.x + px(30.),
                "no empty label column"
            );
            assert!(
                last.bottom() > fields.bottom(),
                "rows must overflow, not compress"
            );
            assert!(
                cx.debug_bounds("local-limit-row-http.envelope_bytes")
                    .is_none()
            );
        }
        let fields = cx.debug_bounds("local-limit-fields").unwrap();
        let actions_before_scroll = cx.debug_bounds("local-limit-actions").unwrap();
        cx.simulate_event(ScrollWheelEvent {
            position: fields.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-5000.))),
            ..Default::default()
        });
        cx.run_until_parked();
        let last = cx.debug_bounds("local-limit-row-http.url_bytes").unwrap();
        assert!(
            last.bottom() <= fields.bottom() + px(1.),
            "last field must be reachable"
        );
        assert_eq!(
            cx.debug_bounds("local-limit-actions").unwrap(),
            actions_before_scroll
        );

        let input = cx.read(|cx| {
            app.read(cx)
                .local_execution_limit_editor
                .as_ref()
                .unwrap()
                .inputs["http.redirects"]
                .clone()
        });
        cx.update(|window, cx| input.update(cx, |input, cx| input.set_value("0", window, cx)));
        // A draft in another section must be retained and saved along with HTTP.
        cx.run_until_parked();
        let scripts_tab = cx.debug_bounds("local-limit-tab-Scripts").unwrap();
        cx.simulate_click(scripts_tab.center(), Modifiers::none());
        cx.run_until_parked();
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let editor = app.local_execution_limit_editor.as_ref().unwrap();
                assert_eq!(editor.section, 2);
                editor.inputs["script.log_entries"]
                    .update(cx, |input, cx| input.set_value("7", window, cx));
            });
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("local-limit-row-script.timeout_ms")
                .is_some()
        );
        assert!(cx.read(|cx| app.read(cx).local_limit_scope_dirty(cx)));
        let scope_picker = cx.debug_bounds("local-limit-scope-action").unwrap();
        let focus_before = cx.update(|window, cx| window.focused(cx));
        cx.simulate_click(scope_picker.center(), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(
            cx.update(|window, cx| window.focused(cx)),
            focus_before,
            "the dirty scope picker must not open a menu"
        );
        let http_tab = cx.debug_bounds("local-limit-tab-HTTP").unwrap();
        cx.simulate_click(http_tab.center(), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(cx.read(|cx| input.read(cx).value().to_string()), "0");
        cx.simulate_click(scripts_tab.center(), Modifiers::none());
        cx.run_until_parked();
        let save = cx
            .debug_bounds("local-limit-save-action")
            .expect("save rendered");
        cx.simulate_click(save.center(), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(
            reload_store
                .load_app_settings()
                .unwrap()
                .execution_limits
                .application["http.redirects"],
            Bound::limited(0)
        );
        assert_eq!(
            reload_store
                .load_app_settings()
                .unwrap()
                .execution_limits
                .application["script.log_entries"],
            Bound::limited(7)
        );
        assert!(!cx.read(|cx| app.read(cx).local_limit_scope_dirty(cx)));

        // Use the real dropdown to select the first local workspace.
        let scope_picker = cx.debug_bounds("local-limit-scope-action").unwrap();
        cx.simulate_click(scope_picker.center(), Modifiers::none());
        cx.run_until_parked();
        cx.simulate_keystrokes("down down enter");
        cx.run_until_parked();
        cx.read(|cx| {
            let app = app.read(cx);
            assert_eq!(
                app.local_execution_limit_editor
                    .as_ref()
                    .unwrap()
                    .scope
                    .workspace
                    .as_ref(),
                Some(&app.local_workspaces[0].id)
            );
        });

        // Invalid text in an inactive section must not partially save the
        // scope. Discard restores every section and unlocks scope selection.
        let saved_settings = reload_store.load_app_settings().unwrap().execution_limits;
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.local_execution_limit_editor.as_ref().unwrap().inputs["http.redirects"]
                    .update(cx, |input, cx| input.set_value("-1", window, cx));
            });
        });
        cx.run_until_parked();
        let save = cx.debug_bounds("local-limit-save-action").unwrap();
        cx.simulate_click(save.center(), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(
            reload_store.load_app_settings().unwrap().execution_limits,
            saved_settings
        );
        assert!(cx.read(|cx| {
            app.read(cx)
                .settings_notice
                .as_deref()
                .unwrap()
                .contains("http.redirects")
        }));
        let discard = cx.debug_bounds("local-limit-discard-action").unwrap();
        cx.simulate_click(discard.center(), Modifiers::none());
        cx.run_until_parked();
        assert!(!cx.read(|cx| app.read(cx).local_limit_scope_dirty(cx)));

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let input =
                    app.local_execution_limit_editor.as_ref().unwrap().inputs["http.redirects"]
                        .clone();
                input.update(cx, |input, cx| input.set_value("Unlimited", window, cx));
            });
            view.update(cx, |_, cx| cx.notify());
        });
        cx.run_until_parked();
        let save = cx.debug_bounds("local-limit-save-action").unwrap();
        cx.simulate_click(save.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        cx.read(|cx| {
            assert!(
                app.read(cx)
                    .local_execution_limits_for_request(None)
                    .unwrap()
                    .get("http.redirects")
                    .unlimited
            )
        });
        let reset = cx.debug_bounds("local-limit-reset-action").unwrap();
        cx.simulate_click(reset.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        cx.read(|cx| {
            assert_eq!(
                app.read(cx)
                    .local_execution_limits_for_request(None)
                    .unwrap()
                    .get("http.redirects"),
                Bound::limited(0)
            )
        });
        assert!(
            reload_store
                .load_app_settings()
                .unwrap()
                .execution_limits
                .workspaces
                .is_empty()
        );
    }
}
