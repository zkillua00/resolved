use super::*;
use crate::core::execution_limits::{Bound, DEFAULT_LIMITS, ExecutionLimits};
use crate::core::local_execution_limits::{Overrides, local_scope_path, validate_overrides};
use gpui_component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use std::collections::BTreeMap;

fn server_only_reason(key: &str) -> Option<&'static str> {
    match key {
        "http.envelope_bytes" | "websocket.opening_bytes" => {
            Some("Server relay envelope only; direct local execution has no relay envelope.")
        }
        "http.tls_handshake_timeout_ms" => {
            Some("Separate TLS timer is server-only; local connect timeout includes TLS.")
        }
        _ => None,
    }
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
            .group(SettingGroup::new().title("Local policies").item(SettingItem::new(
                "Overrides",
                SettingField::<SharedString>::render(move |_, window, cx| {
                    let Some(this) = this.upgrade() else { return div().into_any_element(); };
                    this.update(cx, |this, cx| this.render_local_execution_limits(window, cx))
                }),
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

    fn select_local_limit_scope(
        &mut self,
        scope: Scope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let overrides = self.local_limit_overrides(&scope);
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
                (key.to_string(), input)
            })
            .collect();
        self.local_execution_limit_editor = Some(LocalExecutionLimitEditor { scope, inputs });
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
                    let text = input.read(cx).value().to_string();
                    let text = text.trim();
                    if text.is_empty() || text.eq_ignore_ascii_case("inherit") {
                        continue;
                    }
                    let bound = if text.eq_ignore_ascii_case("unlimited") {
                        Bound::unlimited()
                    } else {
                        Bound::limited(text.parse::<i64>().map_err(|_| {
                            format!("{key}: enter a non-negative integer, Inherit or Unlimited")
                        })?)
                    };
                    values.insert(key.clone(), bound);
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
        let inputs = editor.inputs.clone();
        let mut scopes = vec![("Application".to_owned(), Scope::default())];
        for workspace in &self.local_workspaces {
            scopes.push((
                format!("Workspace: {}", workspace.name),
                Scope {
                    workspace: Some(workspace.id.clone()),
                    ..Default::default()
                },
            ));
        }
        if let WorkspaceProviderId::Local(workspace) = self.workspace_providers.active_id() {
            for collection in &self.workspace.collections {
                let scope = Scope {
                    workspace: Some(workspace.clone()),
                    collection: Some(collection.id.clone()),
                    folder: None,
                };
                scopes.push((format!("Collection: {}", collection.name), scope.clone()));
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
                        format!("{} / {name}", collection.name),
                        Scope {
                            folder: Some(folder.id.clone()),
                            ..scope.clone()
                        },
                    ));
                }
            }
        }
        let mut selector = h_flex().flex_wrap().gap_2();
        for (index, (label, scope)) in scopes.into_iter().enumerate() {
            selector = selector.child(
                Button::new(("local-limit-scope", index))
                    .label(label)
                    .selected(scope == selected)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_local_limit_scope(scope.clone(), window, cx)
                    })),
            );
        }
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
        let mut content = v_flex().w_full().gap_3().child(selector)
            .child(div().text_sm().child("Blank / Inherit uses the nearest ancestor. Enter a non-negative integer (including zero) or Unlimited. Times are milliseconds; sizes are bytes. Switching scope discards unsaved edits."));
        match effective {
            Err(error) => content = content.child(div().child(error)),
            Ok((effective, sources)) => {
                for (index, (key, _)) in DEFAULT_LIMITS.iter().enumerate() {
                    if let Some(reason) = server_only_reason(key) {
                        content = content.child(
                            v_flex()
                                .gap_1()
                                .child(div().text_sm().child(format!("{key} — server only")))
                                .child(div().text_xs().child(reason)),
                        );
                        continue;
                    }
                    let bound = effective.get(key);
                    let value = if bound.unlimited {
                        "Unlimited".into()
                    } else {
                        bound.value.to_string()
                    };
                    let source = sources
                        .get(*key)
                        .map(String::as_str)
                        .unwrap_or("Built-in default");
                    let input = inputs.get(*key).unwrap().clone();
                    let inherit = input.clone();
                    let unlimited = input.clone();
                    content = content.child(
                        v_flex()
                            .gap_1()
                            .child(div().text_sm().child(key.to_string()))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(div().flex_1().child(
                                        Input::new(&input).disabled(!self.settings_writable),
                                    ))
                                    .child(
                                        Button::new(("local-limit-inherit", index))
                                            .label("Inherit")
                                            .disabled(!self.settings_writable)
                                            .on_click(move |_, window, cx| {
                                                inherit.update(cx, |input, cx| {
                                                    input.set_value("", window, cx)
                                                })
                                            }),
                                    )
                                    .child(
                                        Button::new(("local-limit-unlimited", index))
                                            .label("Unlimited")
                                            .disabled(!self.settings_writable)
                                            .on_click(move |_, window, cx| {
                                                unlimited.update(cx, |input, cx| {
                                                    input.set_value("Unlimited", window, cx)
                                                })
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .child(format!("Saved effective: {value} · Source: {source}")),
                            ),
                    );
                }
            }
        }
        content
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .debug_selector(|| "local-limit-save-action".to_owned())
                            .child(
                                Button::new("local-limit-save")
                                    .label("Save")
                                    .disabled(!self.settings_writable)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save_local_limit_scope(false, window, cx)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .debug_selector(|| "local-limit-reset-action".to_owned())
                            .child(
                                Button::new("local-limit-reset")
                                    .label("Reset scope to inheritance")
                                    .disabled(!self.settings_writable)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save_local_limit_scope(true, window, cx)
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, size};

    struct LimitsView(Entity<ApiTester>);
    impl Render for LimitsView {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.0
                .update(cx, |app, cx| app.render_local_execution_limits(window, cx))
        }
    }

    #[gpui::test]
    fn local_limit_editor_saves_and_resets_real_policy(cx: &mut TestAppContext) {
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
        cx.simulate_resize(size(px(1100.), px(2600.)));
        cx.run_until_parked();
        let input = cx.read(|cx| {
            app.read(cx)
                .local_execution_limit_editor
                .as_ref()
                .unwrap()
                .inputs["http.redirects"]
                .clone()
        });
        cx.update(|window, cx| input.update(cx, |input, cx| input.set_value("0", window, cx)));
        cx.run_until_parked();
        let save = cx
            .debug_bounds("local-limit-save-action")
            .expect("save rendered");
        cx.simulate_click(save.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        assert_eq!(
            reload_store
                .load_app_settings()
                .unwrap()
                .execution_limits
                .application["http.redirects"],
            Bound::limited(0)
        );

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let WorkspaceProviderId::Local(id) = app.workspace_providers.active_id() else {
                    panic!("local workspace")
                };
                app.select_local_limit_scope(
                    Scope {
                        workspace: Some(id.clone()),
                        ..Default::default()
                    },
                    window,
                    cx,
                );
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
