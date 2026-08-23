use super::*;

fn environment_workspace_handles_save(
    active_workspace_tab: ActiveWorkspaceTab,
    sidebar_tab: SidebarTab,
) -> bool {
    active_workspace_tab == ActiveWorkspaceTab::Request && sidebar_tab == SidebarTab::Environments
}

fn request_workspace_handles_format(
    active_workspace_tab: ActiveWorkspaceTab,
    sidebar_tab: SidebarTab,
) -> bool {
    active_workspace_tab == ActiveWorkspaceTab::Request && sidebar_tab == SidebarTab::Collections
}

impl ApiTester {
    pub(crate) fn on_new_request_tab(
        &mut self,
        _: &shortcuts::NewRequestTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_blank_request_tab(window, cx);
    }

    pub(crate) fn on_close_request_tab(
        &mut self,
        _: &shortcuts::CloseRequestTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_active_workspace_tab(window, cx);
    }

    pub(crate) fn on_activate_next_request_tab(
        &mut self,
        _: &shortcuts::ActivateNextRequestTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_adjacent_request_tab(1, window, cx);
    }

    pub(crate) fn on_activate_previous_request_tab(
        &mut self,
        _: &shortcuts::ActivatePreviousRequestTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_adjacent_request_tab(-1, window, cx);
    }

    fn activate_adjacent_request_tab(
        &mut self,
        direction: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = self
            .workspace_tabs
            .adjacent_tab(&self.request_tabs, direction);
        self.activate_workspace_tab(tab, window, cx);
    }

    pub(crate) fn on_send_or_cancel_request(
        &mut self,
        _: &shortcuts::SendOrCancelRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sending {
            self.cancel_request(cx);
        } else {
            self.activate_request_workspace(SidebarTab::Collections, window, cx);
            self.start_request(window, cx);
        }
    }

    pub(crate) fn on_save_request(
        &mut self,
        _: &shortcuts::SaveRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if environment_workspace_handles_save(self.workspace_tabs.active(), self.sidebar_tab) {
            // Match the Environment editor's Save button. Command-S belongs
            // to the visible editor and must never fall through to request
            // saving, even when there is nothing writable to save.
            if !self.sending && self.environment_editor_is_dirty(cx) {
                self.save_environment(window, cx);
            }
            return;
        }

        self.activate_request_workspace(SidebarTab::Collections, window, cx);
        self.save_current_request(false, window, cx);
    }

    pub(crate) fn on_save_request_as(
        &mut self,
        _: &shortcuts::SaveRequestAs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_request_workspace(SidebarTab::Collections, window, cx);
        self.save_current_request(true, window, cx);
    }

    pub(crate) fn on_focus_request_url(
        &mut self,
        _: &shortcuts::FocusRequestUrl,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_request_workspace(SidebarTab::Collections, window, cx);
        self.url.read(cx).focus_handle(cx).focus(window);
        cx.notify();
    }

    pub(crate) fn on_format_raw_body(
        &mut self,
        _: &shortcuts::FormatRawBody,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !request_workspace_handles_format(self.workspace_tabs.active(), self.sidebar_tab) {
            return;
        }
        match self.request_pane {
            RequestPane::Body => self.format_raw_body(window, cx),
            RequestPane::PreRequest => {
                let editor = self.pre_request_script.clone();
                self.format_script_editor(editor, "pre-request", window, cx);
            }
            RequestPane::PostResponse => {
                let editor = self.post_response_script.clone();
                self.format_script_editor(editor, "post-response", window, cx);
            }
            RequestPane::Headers => {
                self.request_notice =
                    Some("Open a raw body or script editor to format its buffer.".to_owned());
                cx.notify();
            }
        }
    }

    pub(crate) fn on_show_collections(
        &mut self,
        _: &shortcuts::ShowCollections,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_request_workspace(SidebarTab::Collections, window, cx);
    }

    pub(crate) fn on_show_environments(
        &mut self,
        _: &shortcuts::ShowEnvironments,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_request_workspace(SidebarTab::Environments, window, cx);
    }

    pub(crate) fn on_show_history(
        &mut self,
        _: &shortcuts::ShowHistory,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_request_workspace(SidebarTab::History, window, cx);
    }

    pub(crate) fn on_show_settings(
        &mut self,
        _: &shortcuts::ShowSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_workspace_tool_tab(WorkspaceToolTab::Settings, window, cx);
    }

    pub(crate) fn on_toggle_navigation(
        &mut self,
        _: &shortcuts::ToggleNavigation,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigation_compact = !self.navigation_compact;
        self.persist_navigation_preference(cx);
        cx.notify();
    }

    pub(crate) fn on_toggle_metrics(
        &mut self,
        _: &shortcuts::ToggleMetrics,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.debug_overlay
            .update(cx, |overlay, cx| overlay.toggle(cx));
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, px, size};

    #[test]
    fn save_shortcut_targets_only_the_visible_environment_workspace() {
        assert!(environment_workspace_handles_save(
            ActiveWorkspaceTab::Request,
            SidebarTab::Environments,
        ));

        for active_workspace_tab in [
            ActiveWorkspaceTab::Welcome,
            ActiveWorkspaceTab::RequestProxy,
            ActiveWorkspaceTab::ServerTools,
            ActiveWorkspaceTab::Settings,
            ActiveWorkspaceTab::ThemeCss,
        ] {
            assert!(!environment_workspace_handles_save(
                active_workspace_tab,
                SidebarTab::Environments,
            ));
        }

        for sidebar_tab in [SidebarTab::Collections, SidebarTab::History] {
            assert!(!environment_workspace_handles_save(
                ActiveWorkspaceTab::Request,
                sidebar_tab,
            ));
        }
    }

    #[test]
    fn format_shortcut_targets_only_the_visible_request_workspace() {
        assert!(request_workspace_handles_format(
            ActiveWorkspaceTab::Request,
            SidebarTab::Collections,
        ));

        for active_workspace_tab in [
            ActiveWorkspaceTab::Welcome,
            ActiveWorkspaceTab::RequestProxy,
            ActiveWorkspaceTab::ServerTools,
            ActiveWorkspaceTab::Settings,
            ActiveWorkspaceTab::ThemeCss,
        ] {
            assert!(!request_workspace_handles_format(
                active_workspace_tab,
                SidebarTab::Collections,
            ));
        }
        for sidebar_tab in [SidebarTab::Environments, SidebarTab::History] {
            assert!(!request_workspace_handles_format(
                ActiveWorkspaceTab::Request,
                sidebar_tab,
            ));
        }
    }

    #[gpui::test]
    fn command_s_saves_environment_without_switching_to_requests(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("create temporary database directory");
        let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
        store.initialize().expect("initialize test database");

        let mut workspace = Workspace::default();
        let environment_id = workspace
            .create_environment("Development")
            .expect("create test environment");
        workspace
            .set_active_environment(Some(&environment_id))
            .expect("activate test environment");
        store
            .save_workspace(&workspace)
            .expect("seed test workspace");

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
        cx.run_until_parked();

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.activate_request_workspace(SidebarTab::Environments, window, cx);
                app.environment_name.update(cx, |input, cx| {
                    input.set_value("Staging", window, cx);
                });
                app.environment_name.read(cx).focus_handle(cx).focus(window);
            });
        });
        cx.run_until_parked();
        assert!(cx.update(|_, cx| app.read(cx).environment_editor_is_dirty(cx)));

        cx.simulate_keystrokes("cmd-s");

        let (active_workspace_tab, sidebar_tab, editor_dirty, environment_name) =
            cx.update(|_, cx| {
                let app = app.read(cx);
                (
                    app.workspace_tabs.active(),
                    app.sidebar_tab,
                    app.environment_editor_is_dirty(cx),
                    app.workspace
                        .environment(&environment_id)
                        .expect("environment remains in memory")
                        .name
                        .clone(),
                )
            });
        assert_eq!(active_workspace_tab, ActiveWorkspaceTab::Request);
        assert_eq!(sidebar_tab, SidebarTab::Environments);
        assert!(!editor_dirty);
        assert_eq!(environment_name, "Staging");

        let persisted = store.load_workspace().expect("reload saved workspace");
        assert_eq!(
            persisted
                .environment(&environment_id)
                .expect("environment was persisted")
                .name,
            "Staging"
        );
    }

    #[gpui::test]
    fn command_s_saves_request_to_the_selected_collection(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("create temporary database directory");
        let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
        store.initialize().expect("initialize test database");

        let mut workspace = Workspace::default();
        let collection_id = workspace
            .create_collection("Users")
            .expect("create test collection");
        store
            .save_workspace(&workspace)
            .expect("seed test workspace");

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
        cx.run_until_parked();

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.activate_request_workspace(SidebarTab::Collections, window, cx);
                app.url.update(cx, |input, cx| {
                    input.set_value("https://example.com/users", window, cx);
                });
                app.saved_request_name.update(cx, |input, cx| {
                    input.set_value("List users", window, cx);
                });
            });
        });
        cx.run_until_parked();

        cx.simulate_keystrokes("cmd-s");

        let (request_name, request_url) = cx.update(|_, cx| {
            let app = app.read(cx);
            let request = &app
                .workspace
                .collection(&collection_id)
                .expect("collection remains in memory")
                .requests[0];
            (request.name.clone(), request.definition.request.url.clone())
        });
        assert_eq!(request_name, "List users");
        assert_eq!(request_url, "https://example.com/users");

        let persisted = store.load_workspace().expect("reload saved workspace");
        let request = &persisted
            .collection(&collection_id)
            .expect("collection was persisted")
            .requests[0];
        assert_eq!(request.name, "List users");
        assert_eq!(request.definition.request.url, "https://example.com/users");
    }
}
