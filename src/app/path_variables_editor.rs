use std::collections::BTreeMap;

use super::*;

pub(in crate::app) struct PathVariableRow {
    pub(in crate::app) name: String,
    pub(in crate::app) value: Entity<InputState>,
    pub(in crate::app) _subscription: Subscription,
}

/// The URL owns the names; keep the input entities for surviving names so
/// changing another part of the URL does not discard values or input focus.
pub(in crate::app) fn reconcile_path_variable_rows(
    rows: &mut Vec<PathVariableRow>,
    url: &str,
    mut create: impl FnMut(&str) -> PathVariableRow,
) {
    let mut existing = rows
        .drain(..)
        .map(|row| (row.name.clone(), row))
        .collect::<BTreeMap<_, _>>();
    *rows = crate::core::path_variable_names(url)
        .into_iter()
        .map(|name| existing.remove(&name).unwrap_or_else(|| create(&name)))
        .collect();
}

pub(in crate::app) fn path_variable_values(
    rows: &[PathVariableRow],
    cx: &App,
) -> BTreeMap<String, String> {
    rows.iter()
        .filter_map(|row| {
            let value = row.value.read(cx).value().to_string();
            // Empty inputs represent unset values, not an implicit empty path.
            (!value.is_empty()).then(|| (row.name.clone(), value))
        })
        .collect()
}

impl ApiTester {
    fn new_path_variable_row(
        &self,
        name: &str,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PathVariableRow {
        let catalog = Rc::clone(&self.template_variable_catalog);
        let input = cx.new(|cx| {
            template_input_state(window, cx, catalog, "Required value", value.to_owned())
        });
        let subscription = cx.subscribe(&input, |this, input, event, cx| {
            this.track_template_input_focus(&input, event);
            if matches!(event, InputEvent::Change) {
                this.schedule_template_input_refresh(&input, cx);
                this.refresh_request_dirty_part(RequestDirtyPart::PathVariables, cx);
                cx.notify();
            }
        });
        self.refresh_template_input(&input, cx);
        PathVariableRow {
            name: name.to_owned(),
            value: input,
            _subscription: subscription,
        }
    }

    pub(super) fn sync_path_variables_from_url(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let url = self.url.read(cx).value().to_string();
        let mut rows = std::mem::take(&mut self.path_variables);
        reconcile_path_variable_rows(&mut rows, &url, |name| {
            self.new_path_variable_row(name, "", window, cx)
        });
        self.path_variables = rows;
        self.refresh_request_dirty_part(RequestDirtyPart::PathVariables, cx);
        cx.notify();
    }

    pub(super) fn load_path_variables(
        &mut self,
        values: &BTreeMap<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let url = self.url.read(cx).value().to_string();
        let mut rows = Vec::new();
        reconcile_path_variable_rows(&mut rows, &url, |name| {
            self.new_path_variable_row(
                name,
                values.get(name).map(String::as_str).unwrap_or_default(),
                window,
                cx,
            )
        });
        self.path_variables = rows;
    }
}

pub(in crate::app) fn render_path_variables_editor(
    rows: &[PathVariableRow],
    id: SharedString,
    cx: &App,
) -> AnyElement {
    let missing_count = rows
        .iter()
        .filter(|row| row.value.read(cx).value().is_empty())
        .count();
    v_flex()
        .size_full()
        .min_h_0()
        .overflow_hidden()
        .bg(cx.api_surface())
        .child(
            h_flex()
                .h(px(42.))
                .w_full()
                .flex_shrink_0()
                .px_3()
                .gap_2()
                .bg(cx.api_surface_low())
                .child(div().text_sm().font_semibold().child("Path Variables"))
                .when(missing_count > 0, |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().warning)
                            .child(format!("{missing_count} missing")),
                    )
                }),
        )
        .child(
            v_flex()
                .flex_shrink_0()
                .px_3()
                .py_2()
                .gap_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("Use {name} in the URL path, e.g. /users/{user_id}. Edit the URL to add or remove variables.")
                .child("Values are encoded as path data and can use {{environment_variables}}."),
        )
        .child(
            h_flex()
                .h(px(34.))
                .w_full()
                .flex_shrink_0()
                .bg(cx.api_surface_low())
                .text_xs()
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child(query_params_editor::query_param_cell(cx).child(div().px_3().child("NAME")))
                .child(query_params_editor::query_param_cell(cx).child(div().px_3().child("VALUE"))),
        )
        .child(
            v_flex()
                .id(id)
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .when(rows.is_empty(), |this| {
                    this.child(
                        div()
                            .px_3()
                            .py_4()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("No path variables in this URL."),
                    )
                })
                .children(rows.iter().map(|row| {
                    h_flex()
                        .id(SharedString::from(format!("path-variable-{}", row.name)))
                        .w_full()
                        .h(px(44.))
                        .flex_shrink_0()
                        .border_t_1()
                        .border_color(cx.api_outline_variant())
                        .child(
                            query_params_editor::query_param_cell(cx).child(
                                div().px_3().text_sm().truncate().child(row.name.clone()),
                            ),
                        )
                        .child(query_params_editor::query_param_input_cell(&row.value, cx))
                })),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn path_variables_follow_url_and_survive_tab_switches(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().expect("temporary database directory");
        let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
        store.initialize().expect("initialize database");
        let mut app = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            let bindings = shortcuts::capture_base_key_bindings(cx);
            crate::theme::configure(cx);
            let view = cx
                .new(|cx| ApiTester::new_with_database_store(bindings, store.clone(), window, cx));
            app = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        let app = app.expect("app entity");
        cx.simulate_resize(gpui::size(px(1_200.), px(800.)));

        let original_tab = cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_blank_request_tab(window, cx);
                let mut draft = RequestDraft::new(
                    "GET",
                    "https://example.test/users/{user_id}/posts/{post_id}?q={literal}",
                );
                draft.path_variables.insert("user_id".into(), "42".into());
                app.load_template_unchecked(
                    RequestTemplate {
                        request: draft,
                        ..RequestTemplate::default()
                    },
                    None,
                    None,
                    window,
                    cx,
                );
                app.request_pane = RequestPane::PathVariables;
                assert!(!app.request_part_is_dirty(RequestDirtyPart::PathVariables, cx));
                app.request_tabs.active_tab_id().clone()
            })
        });
        cx.run_until_parked();

        let user_value = cx.update(|_, cx| {
            let app = app.read(cx);
            assert_eq!(
                app.path_variables
                    .iter()
                    .map(|row| row.name.as_str())
                    .collect::<Vec<_>>(),
                ["user_id", "post_id"]
            );
            assert_eq!(app.path_variables[0].value.read(cx).value().as_ref(), "42");
            assert!(app.path_variables[1].value.read(cx).value().is_empty());
            assert_eq!(app.draft(cx).path_variables.len(), 1);
            app.path_variables[0].value.clone()
        });
        cx.update(|window, cx| {
            user_value.update(cx, |input, cx| input.set_value("a/b ?", window, cx));
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                assert!(app.request_part_is_dirty(RequestDirtyPart::PathVariables, cx));
                assert_eq!(app.draft(cx).path_variables["user_id"], "a/b ?");
                assert!(app.url.read(cx).value().contains("{user_id}"));
                app.url.update(cx, |input, cx| {
                    input.set_value(
                        "https://example.test/users/{user_id}/again/{user_id}?q={literal}#/{ignored}",
                        window,
                        cx,
                    );
                });
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                assert_eq!(app.path_variables.len(), 1);
                assert_eq!(
                    app.path_variables[0].value.entity_id(),
                    user_value.entity_id()
                );
                assert_eq!(app.draft(cx).path_variables["user_id"], "a/b ?");
                app.open_blank_request_tab(window, cx);
                assert!(app.path_variables.is_empty());
                app.activate_request_tab(original_tab.clone(), window, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let app = app.read(cx);
            assert_eq!(app.request_pane, RequestPane::PathVariables);
            assert_eq!(app.path_variables.len(), 1);
            assert_eq!(app.draft(cx).path_variables["user_id"], "a/b ?");
            let resolved = resolve_request(&app.draft(cx), None).expect("resolve URL");
            assert_eq!(
                resolved.request.url,
                "https://example.test/users/a%2Fb%20%3F/again/a%2Fb%20%3F?q={literal}#/{ignored}",
            );
            assert!(resolved.request.path_variables.is_empty());
        });

        // OpenAPI parameters without examples are still unset. Hydrating and
        // switching an imported tab must not turn that representation into edits.
        let imported_tab = cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let imported = import_requests(
                    r#"{"openapi":"3.0.3","info":{"title":"Users","version":"1"},"paths":{"/users/{id}":{"get":{"parameters":[{"in":"path","name":"id","required":true,"schema":{"type":"string"}}],"responses":{}}}}}"#,
                ).expect("import OpenAPI");
                let opened = app.request_tabs.open_saved(
                    "Imported user",
                    imported.requests[0].template.clone(),
                    RequestTabAssociation::new(None, None, Some("imported-user".into())),
                );
                app.restore_active_request_tab(window, cx);
                assert!(!app.request_is_dirty());
                assert_eq!(app.path_variables.len(), 1);
                assert!(app.draft(cx).path_variables.is_empty());
                opened.tab_id
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_blank_request_tab(window, cx);
                app.activate_request_tab(imported_tab, window, cx);
                assert!(!app.request_is_dirty());
            });
        });
    }
}
