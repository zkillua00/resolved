use super::*;
use crate::core::execution_limits::Bound;
use crate::core::{
    ExecutionLimitScope, ExecutionLimitSnapshot, ExecutionLimitSource, load_execution_limits,
    replace_execution_limits,
};
use gpui_component::tab::Tab;

const SERVER_LIMIT_SECTIONS: &[(&str, &str, &str)] = &[
    (
        "HTTP",
        "http.",
        "Request timeouts, connection budgets, payload sizes, redirects, and relay envelopes.",
    ),
    (
        "WebSocket",
        "websocket.",
        "Connection, message, session, and automation budgets.",
    ),
    (
        "Scripts",
        "script.",
        "Runtime, memory, source, output, and logging budgets for scripts.",
    ),
    (
        "Chains",
        "chain.",
        "Depth and request budgets for chained request execution.",
    ),
];

#[derive(Clone, Debug, Default)]
pub(super) struct ExecutionLimitState {
    scope: ExecutionLimitScope,
    snapshot: Option<ExecutionLimitSnapshot>,
    draft: BTreeMap<String, Bound>,
    section: usize,
    busy: bool,
    error: Option<String>,
    notice: Option<String>,
    request_token: Option<Rc<()>>,
}

impl ExecutionLimitState {
    fn dirty(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.overrides != self.draft)
    }
}

fn bound_label(bound: &Bound, unit: &str) -> String {
    if bound.unlimited {
        "Unlimited".to_owned()
    } else {
        format!("{} {unit}", bound.value)
    }
}

fn source_label(source: &ExecutionLimitSource, scopes: &[(ExecutionLimitScope, String)]) -> String {
    let scope = match source.kind.as_str() {
        "default" => return "Built-in default".to_owned(),
        "deployment" => return "Deployment".to_owned(),
        "workspace" => source
            .workspace_id
            .clone()
            .map(ExecutionLimitScope::Workspace),
        "collection" => source.collection_id.as_ref().and_then(|id| {
            scopes.iter().find_map(|(scope, _)| match scope {
                ExecutionLimitScope::Collection { collection_id, .. } if collection_id == id => {
                    Some(scope.clone())
                }
                _ => None,
            })
        }),
        _ => None,
    };
    scope
        .as_ref()
        .and_then(|scope| scopes.iter().find(|(candidate, _)| candidate == scope))
        .map(|(_, label)| label.clone())
        .unwrap_or_else(|| {
            format!(
                "{} {}",
                source.kind,
                source
                    .collection_id
                    .as_deref()
                    .or(source.workspace_id.as_deref())
                    .unwrap_or("")
            )
        })
}

fn collect_scopes(
    collections: &[UpstreamCollectionView],
    prefix: &str,
    scopes: &mut Vec<(ExecutionLimitScope, String)>,
) {
    for collection in collections {
        let label = format!("{prefix} / {}", collection.name);
        scopes.push((
            ExecutionLimitScope::Collection {
                workspace_id: collection.workspace_id.clone(),
                collection_id: collection.id.clone(),
            },
            label.clone(),
        ));
        collect_scopes(&collection.sub_collections, &label, scopes);
    }
}

fn parse_custom_bound(value: &str) -> Result<Bound, String> {
    let value = value
        .trim()
        .parse::<i64>()
        .map_err(|_| "Enter a whole number between 0 and 9223372036854775807.".to_owned())?;
    if value < 0 {
        return Err("Execution limits cannot be negative.".to_owned());
    }
    Ok(Bound {
        unlimited: false,
        value,
    })
}

impl ApiTester {
    pub(in crate::app) fn refresh_execution_limits_realtime(
        &mut self,
        upstream_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.server_management.upstream_id.as_deref() != Some(upstream_id) {
            return;
        }
        let state = &mut self.server_management.execution_limits;
        if state.busy || state.snapshot.is_none() {
            return;
        }
        if state.dirty() {
            state.notice = Some("Server limits changed while you were editing. Your draft is retained; discard edits and select the scope again to reload before making further changes.".to_owned());
            cx.notify();
            return;
        }
        let scope = state.scope.clone();
        self.request_execution_limits(scope, false, window, cx);
    }

    fn execution_limits_can_update(&self) -> bool {
        self.server_management
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| {
                snapshot.has_permission(match &self.server_management.execution_limits.scope {
                    ExecutionLimitScope::Deployment => SERVER_SETTINGS_UPDATE,
                    ExecutionLimitScope::Workspace(_) => crate::core::WORKSPACES_UPDATE,
                    ExecutionLimitScope::Collection { .. } => crate::core::COLLECTIONS_UPDATE,
                })
            })
    }

    pub(super) fn render_execution_limits(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.server_management.execution_limits;
        let mut scopes = vec![(ExecutionLimitScope::Deployment, "Deployment".to_owned())];
        if let Some(workspaces) = self
            .server_management
            .snapshot
            .as_ref()
            .and_then(|s| s.workspaces.as_ref())
        {
            for workspace in workspaces {
                let label = format!("Workspace: {}", workspace.name);
                scopes.push((
                    ExecutionLimitScope::Workspace(workspace.id.clone()),
                    label.clone(),
                ));
                collect_scopes(&workspace.collections, &label, &mut scopes);
            }
        }
        // The full management tree requires request-read permission too. Workspace
        // summaries still let workspace-only roles select their accessible scopes.
        if let Some(profile) = self.settings.upstreams.active() {
            for workspace in &profile.workspaces {
                let scope = ExecutionLimitScope::Workspace(workspace.id.clone());
                if !scopes.iter().any(|(candidate, _)| candidate == &scope) {
                    scopes.push((scope, format!("Workspace: {}", workspace.name)));
                }
            }
        }
        if !scopes.iter().any(|(scope, _)| scope == &state.scope) {
            let label = match &state.scope {
                ExecutionLimitScope::Deployment => "Deployment".to_owned(),
                ExecutionLimitScope::Workspace(id) => format!("Workspace ID: {id}"),
                ExecutionLimitScope::Collection {
                    workspace_id,
                    collection_id,
                } => format!("Workspace ID: {workspace_id} / Collection ID: {collection_id}"),
            };
            scopes.push((state.scope.clone(), label));
        }
        let can_update = self.execution_limits_can_update();
        let unavailable =
            self.server_management.status.busy() || self.server_management.snapshot.is_none();
        let scope_label = scopes
            .iter()
            .find(|(scope, _)| scope == &state.scope)
            .map(|(_, label)| label.clone())
            .unwrap_or_else(|| "Scope no longer available".to_owned());
        let menu_scopes = scopes.clone();
        let menu_selected = state.scope.clone();
        let menu_this = cx.entity().downgrade();
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
                Button::new("execution-limit-scope")
                    .label("Change scope")
                    .outline()
                    .dropdown_caret(true)
                    .disabled(state.busy || unavailable || state.dirty())
                    .tooltip(if state.dirty() {
                        "Save or discard your edits before switching scope.".to_owned()
                    } else {
                        scope_label
                    })
                    .dropdown_menu(move |mut menu, _, _| {
                        menu = menu.scrollable(true);
                        for (scope, label) in &menu_scopes {
                            let scope = scope.clone();
                            let this = menu_this.clone();
                            let is_selected = scope == menu_selected;
                            menu = menu.item(
                                PopupMenuItem::new(label.clone())
                                    .checked(is_selected)
                                    .on_click(move |_, window, cx| {
                                        if !is_selected && let Some(this) = this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.request_execution_limits(
                                                    scope.clone(),
                                                    false,
                                                    window,
                                                    cx,
                                                );
                                            });
                                        }
                                    }),
                            );
                        }
                        menu
                    }),
            )
            .child(
                Button::new("execution-limit-scope-by-id")
                    .icon(IconName::Search)
                    .small()
                    .ghost()
                    .tooltip("Open workspace / collection by ID…")
                    .disabled(state.busy || unavailable || state.dirty())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_execution_limit_scope_dialog(window, cx);
                    })),
            );
        let active_section = state.section.min(SERVER_LIMIT_SECTIONS.len() - 1);
        let mut section = v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .child(
                v_flex()
                    .flex_shrink_0()
                    .gap_3()
                    .px_4()
                    .pt_4()
                    .child(selector)
                    .child(
                        TabBar::new("server-limit-sections")
                            .underline()
                            .children(
                                SERVER_LIMIT_SECTIONS
                                    .iter()
                                    .map(|(title, _, _)| Tab::new().label(*title)),
                            )
                            .selected_index(active_section)
                            .on_click(cx.listener(|this, index: &usize, _, cx| {
                                this.server_management.execution_limits.section = *index;
                                cx.notify();
                            })),
                    ),
            );
        let mut fields = v_flex()
            .id("server-limit-fields")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_y_scroll()
            .px_4()
            .child(
                v_flex()
                    .flex_shrink_0()
                    .gap_2()
                    .py_4()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(SERVER_LIMIT_SECTIONS[active_section].2)
                    .child("Blank / Inherit uses the nearest ancestor. Zero is an explicit limit; Unlimited removes the cap. Effective values below reflect the last saved server snapshot.")
                    .child("Workspace and collection scopes follow the same hierarchy used for proxy assignments."),
            );
        if let Some(error) = &state.error {
            fields = fields.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(error.clone()),
            );
        }
        if let Some(notice) = &state.notice {
            fields = fields.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().info)
                    .child(notice.clone()),
            );
        }
        if state.busy {
            fields = fields.child(div().text_sm().child("Loading execution limits…"));
        }
        if let Some(snapshot) = &state.snapshot {
            if !can_update {
                fields = fields.child(
                    div()
                        .text_sm()
                        .child("Read only: you do not have permission to update this scope."),
                );
            }
            for (index, definition) in snapshot.definitions.iter().enumerate() {
                let key = &definition.key;
                if !key.starts_with(SERVER_LIMIT_SECTIONS[active_section].1) {
                    continue;
                }
                let current = state.draft.get(key);
                let effective = snapshot
                    .effective
                    .get(key)
                    .map(|b| bound_label(b, &definition.unit))
                    .unwrap_or_else(|| "Not supplied by server".to_owned());
                let source = snapshot
                    .sources
                    .get(key)
                    .map(|s| source_label(s, &scopes))
                    .unwrap_or_else(|| "Source not supplied".to_owned());
                let inherit_key = key.clone();
                let unlimited_key = key.clone();
                let custom_key = key.clone();
                let custom_label = definition.label.clone();
                let custom_unit = definition.unit.clone();
                let custom_value = current
                    .or(snapshot.effective.get(key))
                    .unwrap_or(&definition.default)
                    .value;
                fields = fields.child(
                    v_flex()
                        .flex_shrink_0()
                        .gap_2()
                        .py_4()
                        .border_t_1()
                        .border_color(cx.api_outline_variant())
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap_3()
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w(rems(14.))
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_medium()
                                                .child(definition.label.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(key.clone()),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .child(
                                            Button::new(("limit-custom", index))
                                                .label(
                                                    current
                                                        .filter(|bound| !bound.unlimited)
                                                        .map(|bound| {
                                                            bound_label(bound, &definition.unit)
                                                        })
                                                        .unwrap_or_else(|| "Custom…".to_owned()),
                                                )
                                                .small()
                                                .outline()
                                                .selected(current.is_some_and(|b| !b.unlimited))
                                                .disabled(!can_update || state.busy)
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.open_execution_limit_dialog(
                                                            custom_key.clone(),
                                                            custom_label.clone(),
                                                            custom_unit.clone(),
                                                            custom_value,
                                                            window,
                                                            cx,
                                                        );
                                                    },
                                                )),
                                        )
                                        .child(
                                            Button::new(("limit-inherit", index))
                                                .label("Inherit")
                                                .small()
                                                .ghost()
                                                .selected(current.is_none())
                                                .disabled(!can_update || state.busy)
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.server_management
                                                        .execution_limits
                                                        .draft
                                                        .remove(&inherit_key);
                                                    this.server_management
                                                        .execution_limits
                                                        .notice = None;
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            Button::new(("limit-unlimited", index))
                                                .label("Unlimited")
                                                .small()
                                                .ghost()
                                                .selected(current.is_some_and(|b| b.unlimited))
                                                .disabled(!can_update || state.busy)
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.server_management
                                                        .execution_limits
                                                        .draft
                                                        .insert(
                                                            unlimited_key.clone(),
                                                            Bound {
                                                                unlimited: true,
                                                                value: 0,
                                                            },
                                                        );
                                                    this.server_management
                                                        .execution_limits
                                                        .notice = None;
                                                    cx.notify();
                                                })),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "Saved effective: {effective} · From {source} · Default: {}",
                                    bound_label(&definition.default, &definition.unit)
                                )),
                        ),
                );
            }
            section = section
                .child(fields)
                .child(
                v_flex()
                    .flex_shrink_0()
                    .gap_2()
                    .p_4()
                    .border_t_1()
                    .border_color(cx.api_outline_variant())
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(if !can_update {
                                "Read only — you do not have permission to update this scope."
                            } else if state.dirty() {
                                "Unsaved changes — save or discard before switching scope."
                            } else {
                                "Saved limits apply immediately. More specific scopes take precedence."
                            }),
                    )
                    .child(
                        h_flex()
                    .gap_2()
                    .child(div().flex_1())
                    .child(
                        Button::new("reset-execution-limit-draft")
                            .label("Discard edits")
                            .ghost()
                            .disabled(state.busy || !state.dirty())
                            .on_click(cx.listener(|this, _, _, cx| {
                                let state = &mut this.server_management.execution_limits;
                                if let Some(snapshot) = &state.snapshot {
                                    state.draft = snapshot.overrides.clone();
                                }
                                state.error = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("save-execution-limits")
                            .label("Save changes")
                            .primary()
                            .disabled(!can_update || state.busy || !state.dirty())
                            .on_click(cx.listener(|this, _, window, cx| {
                                let scope = this.server_management.execution_limits.scope.clone();
                                this.request_execution_limits(scope, true, window, cx);
                            })),
                    ),
                    ),
            );
        } else {
            section = section.child(fields);
        }
        section.into_any_element()
    }

    fn request_execution_limits(
        &mut self,
        scope: ExecutionLimitScope,
        save: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.server_management.execution_limits.busy
            || (save && !self.execution_limits_can_update())
        {
            return;
        }
        let Some(profile) = self.settings.upstreams.active().cloned() else {
            return;
        };
        let Some(base_url) = profile.parsed_base_url() else {
            return;
        };
        let upstream_id = profile.id.clone();
        let state = &mut self.server_management.execution_limits;
        let request_token = Rc::new(());
        state.request_token = Some(request_token.clone());
        let overrides = save.then(|| state.draft.clone());
        if state.scope != scope {
            state.snapshot = None;
            state.draft.clear();
        }
        state.scope = scope.clone();
        state.busy = true;
        state.error = None;
        state.notice = None;
        let vault = self.credential_vault.clone();
        let client = self.upstream_client.clone();
        let task_upstream_id = upstream_id.clone();
        let task_scope = scope.clone();
        let task = self.runtime.spawn(async move {
            let credential = crate::io::run(move || vault.load_upstream(&task_upstream_id))
                .await
                .map_err(|error| format!("Could not open the saved session: {error}"))?
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Log in to this server again.".to_owned())?;
            if let Some(overrides) = overrides {
                replace_execution_limits(
                    &client,
                    &base_url,
                    credential.bearer_token(),
                    &task_scope,
                    &overrides,
                )
                .await
                .map_err(limit_error)?;
            }
            load_execution_limits(&client, &base_url, credential.bearer_token(), &task_scope)
                .await
                .map_err(limit_error)
        });
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, _, cx| {
                if !this
                    .server_management
                    .execution_limits
                    .request_token
                    .as_ref()
                    .is_some_and(|token| Rc::ptr_eq(token, &request_token))
                    || this.settings.upstreams.active_upstream_id.as_deref()
                        != Some(upstream_id.as_str())
                    || this.server_management.execution_limits.scope != scope
                {
                    return;
                }
                let state = &mut this.server_management.execution_limits;
                state.busy = false;
                match result {
                    Ok(Ok(snapshot)) => {
                        state.draft = snapshot.overrides.clone();
                        state.snapshot = Some(snapshot);
                        state.notice =
                            save.then(|| "Execution limits saved and reloaded.".to_owned());
                    }
                    Ok(Err(error)) => state.error = Some(error),
                    Err(error) => {
                        state.error = Some(format!("Could not load execution limits: {error}"))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_execution_limit_scope_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let workspace =
            cx.new(|cx| InputState::new(window, cx).placeholder("Workspace ID (required)"));
        let collection =
            cx.new(|cx| InputState::new(window, cx).placeholder("Collection ID (optional)"));
        let upstream_id = self.settings.upstreams.active_upstream_id.clone();
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let workspace = workspace.clone();
            let collection = collection.clone();
            let save_workspace = workspace.clone();
            let save_collection = collection.clone();
            let upstream_id = upstream_id.clone();
            let this = this.clone();
            dialog.title("Execution limit scope").confirm()
                .button_props(DialogButtonProps::default().ok_text("Load scope"))
                .child(v_flex().gap_3()
                    .child(div().text_sm().child("For roles that cannot browse the full request tree. Access is checked by the server. Leave Collection ID empty to manage the workspace."))
                    .child(Input::new(&workspace))
                    .child(Input::new(&collection)))
                .on_ok(move |_, window, cx| {
                    let workspace_id = save_workspace.read(cx).value().trim().to_owned();
                    let collection_id = save_collection.read(cx).value().trim().to_owned();
                    if let Some(this) = this.upgrade() {
                        this.update(cx, |this, cx| {
                            if this.settings.upstreams.active_upstream_id != upstream_id { return; }
                            if workspace_id.is_empty() {
                                this.server_management.execution_limits.error = Some("A workspace ID is required to load this scope.".to_owned());
                                cx.notify();
                                return;
                            }
                            let scope = if collection_id.is_empty() {
                                ExecutionLimitScope::Workspace(workspace_id.clone())
                            } else {
                                ExecutionLimitScope::Collection { workspace_id: workspace_id.clone(), collection_id: collection_id.clone() }
                            };
                            this.request_execution_limits(scope, false, window, cx);
                        });
                    }
                    true
                })
        });
    }

    fn open_execution_limit_dialog(
        &mut self,
        key: String,
        label: String,
        unit: String,
        value: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(value.to_string())
                .placeholder("Non-negative whole number")
        });
        let scope = self.server_management.execution_limits.scope.clone();
        let upstream_id = self.settings.upstreams.active_upstream_id.clone();
        let generation = self.server_management_generation;
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let input = input.clone();
            let save_input = input.clone();
            let this = this.clone();
            let key = key.clone();
            let scope = scope.clone();
            let upstream_id = upstream_id.clone();
            dialog.title(format!("{label} ({unit})")).confirm()
                .button_props(DialogButtonProps::default().ok_text("Use value"))
                .child(v_flex().gap_3().child(Input::new(&input))
                    .child(div().text_sm().child("Use a non-negative whole number. Zero is explicit, not Unlimited. The server validates supported keys and ranges when saving.")))
                .on_ok(move |_, _, cx| {
                    let value = parse_custom_bound(save_input.read(cx).value().as_ref());
                    if let Some(this) = this.upgrade() {
                        this.update(cx, |this, cx| {
                            if this.server_management_generation != generation
                                || this.settings.upstreams.active_upstream_id != upstream_id
                                || this.server_management.execution_limits.scope != scope { return; }
                            let state = &mut this.server_management.execution_limits;
                            match &value {
                                Ok(bound) => { state.draft.insert(key.clone(), bound.clone()); state.error = None; state.notice = None; }
                                Err(error) => state.error = Some(error.clone()),
                            }
                            cx.notify();
                        });
                    }
                    // Close on invalid input as well so the inline validation message is visible.
                    true
                })
        });
    }
}

fn limit_error(error: crate::core::UpstreamManagementError) -> String {
    match error {
        crate::core::UpstreamManagementError::Rejected { status, .. } if status == reqwest::StatusCode::NOT_FOUND =>
            "Execution limits are unavailable: this server may not support the limits endpoint, or the selected scope no longer exists. No successful save was confirmed.".to_owned(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_custom_and_invalid_numbers_are_rejected() {
        assert_eq!(
            parse_custom_bound("0").unwrap(),
            Bound {
                unlimited: false,
                value: 0
            }
        );
        assert!(parse_custom_bound("-1").is_err());
        assert!(parse_custom_bound("9223372036854775808").is_err());
        assert!(parse_custom_bound("1.5").is_err());
    }

    #[test]
    fn reset_means_absence_not_zero_or_unlimited() {
        let mut draft = BTreeMap::new();
        draft.insert("key".to_owned(), parse_custom_bound("0").unwrap());
        assert!(draft.contains_key("key"));
        draft.insert(
            "key".to_owned(),
            Bound {
                unlimited: true,
                value: 0,
            },
        );
        assert!(draft["key"].unlimited);
        draft.remove("key");
        assert!(draft.is_empty());
    }

    #[test]
    fn editing_and_resetting_do_not_change_the_authoritative_snapshot() {
        let snapshot: ExecutionLimitSnapshot = serde_json::from_value(serde_json::json!({
            "overrides": {"limit": {"unlimited": false, "value": 0}},
            "effective": {"limit": {"unlimited": false, "value": 0}},
            "sources": {"limit": {"kind": "workspace", "workspace_id": "w"}},
            "definitions": []
        }))
        .unwrap();
        let mut state = ExecutionLimitState {
            draft: snapshot.overrides.clone(),
            snapshot: Some(snapshot.clone()),
            ..Default::default()
        };
        assert!(!state.dirty());
        state.draft.remove("limit");
        assert!(state.dirty());
        assert_eq!(state.snapshot.as_ref().unwrap(), &snapshot);
        state.draft = snapshot.overrides;
        assert!(!state.dirty());
    }

    #[test]
    fn source_labels_distinguish_ancestor_collections_from_selected_scope() {
        let scopes = vec![(
            ExecutionLimitScope::Collection {
                workspace_id: "w".to_owned(),
                collection_id: "parent".to_owned(),
            },
            "Workspace: Shared / Parent".to_owned(),
        )];
        assert_eq!(
            source_label(
                &ExecutionLimitSource {
                    kind: "collection".to_owned(),
                    workspace_id: Some("w".to_owned()),
                    collection_id: Some("parent".to_owned()),
                },
                &scopes
            ),
            "Workspace: Shared / Parent"
        );
        assert_eq!(
            source_label(
                &ExecutionLimitSource {
                    kind: "default".to_owned(),
                    workspace_id: None,
                    collection_id: None,
                },
                &scopes
            ),
            "Built-in default"
        );
    }
}
