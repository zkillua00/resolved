use super::*;

impl ApiTester {
    /// Apply the persisted editor preferences to every currently open code
    /// editor. This is also called after startup because the core editors are
    /// constructed before settings are loaded from SQLite.
    pub(super) fn apply_code_editor_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let settings = self.settings.editor.clone();
        let mut editors = vec![
            self.body.clone(),
            self.pre_request_script.clone(),
            self.post_response_script.clone(),
            self.response_editor.clone(),
            self.snippet_editor.editor.clone(),
            self.snippet_editor.preview_editor.clone(),
        ];
        editors.extend(
            self.theme_editors
                .values()
                .map(|session| session.editor.clone()),
        );
        editors.extend(
            self.pane_editors
                .values()
                .flat_map(|session| session.code_editors()),
        );

        for editor in editors {
            editor.update(cx, |editor, cx| {
                editor.apply_editor_settings(&settings, window, cx);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, px, size};

    #[gpui::test]
    fn persisted_and_live_indentation_settings_reach_the_open_editor(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("create temporary settings directory");
        let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
        store.initialize().expect("initialize test database");
        let mut settings = AppSettings::default();
        settings.editor.set_tab_size(4);
        store
            .save_app_settings(&settings)
            .expect("seed editor settings");

        let mut app = None;
        let store_for_app = store.clone();
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            let base_key_bindings = shortcuts::capture_base_key_bindings(cx);
            crate::theme::configure(cx);
            let view = cx.new(|cx| {
                ApiTester::new_with_database_store(base_key_bindings, store_for_app, window, cx)
            });
            crate::register_app_action_handlers(&view, cx);
            app = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        let app = app.expect("capture app entity");
        cx.update(|window, _| window.activate_window());
        cx.simulate_resize(size(px(1_200.), px(800.)));

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.workspace_tabs.open_tool(WorkspaceToolTab::Snippets);
                app.snippet_editor.editor.update(cx, |editor, cx| {
                    editor.set_value("", window, cx);
                });
                app.snippet_editor
                    .editor
                    .read(cx)
                    .focus_handle(cx)
                    .focus(window);
                cx.notify();
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("tab");
        assert_eq!(
            cx.update(|_, cx| {
                app.read(cx)
                    .snippet_editor
                    .editor
                    .read(cx)
                    .value(cx)
                    .to_string()
            }),
            "    ",
            "persisted editor settings reach the initial snippet editor"
        );

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.workspace_tabs.activate_request();
                app.request_pane = RequestPane::Body;
                app.body.update(cx, |editor, cx| {
                    editor.set_value("", window, cx);
                });
                app.body.read(cx).focus_handle(cx).focus(window);
                cx.notify();
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("tab");
        assert_eq!(
            cx.update(|_, cx| app.read(cx).body.read(cx).value(cx).to_string()),
            "    "
        );

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.set_editor_hard_tabs(true, window, cx);
                app.body.update(cx, |editor, cx| {
                    editor.set_value("", window, cx);
                });
                app.body.read(cx).focus_handle(cx).focus(window);
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("tab");
        assert_eq!(
            cx.update(|_, cx| app.read(cx).body.read(cx).value(cx).to_string()),
            "\t"
        );

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.workspace_tabs.open_tool(WorkspaceToolTab::Snippets);
                app.replace_snippet_source_editor(
                    SnippetCategory::PostResponse,
                    SnippetKind::Executable,
                    String::new(),
                    window,
                    cx,
                );
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("tab");
        assert_eq!(
            cx.update(|_, cx| {
                app.read(cx)
                    .snippet_editor
                    .editor
                    .read(cx)
                    .value(cx)
                    .to_string()
            }),
            "\t",
            "live editor settings reach a replacement snippet editor"
        );
        assert!(
            store
                .load_app_settings()
                .expect("reload editor settings")
                .editor
                .hard_tabs
        );
    }
}
