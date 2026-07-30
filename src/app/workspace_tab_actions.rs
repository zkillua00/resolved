use super::*;

impl ApiTester {
    /// Open or reactivate one of the runtime-only singleton workspace tools.
    ///
    /// The request editor remains the durable source of truth underneath tool
    /// tabs, so snapshot it once when leaving the request surface. Preview is
    /// torn down while hidden to avoid retaining a WKWebView unnecessarily.
    pub(super) fn open_workspace_tool_tab(
        &mut self,
        tab: WorkspaceToolTab,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_template_variable_popover();
        if self.workspace_tabs.active() == ActiveWorkspaceTab::Request {
            self.snapshot_active_request_tab(cx);
            self.hide_preview(cx);
        } else if self.workspace_tabs.active() == ActiveWorkspaceTab::Settings
            && tab != WorkspaceToolTab::Settings
        {
            self.cancel_shortcut_recording(cx);
        }

        self.workspace_tabs.open_tool(tab);
        cx.notify();
    }

    /// Return to a request-backed navigation surface without closing any open
    /// singleton tool tab.
    pub(super) fn activate_request_workspace(
        &mut self,
        sidebar_tab: SidebarTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_template_variable_popover();
        let was_tool = self.workspace_tabs.active() != ActiveWorkspaceTab::Request;
        let was_environment = self.sidebar_tab == SidebarTab::Environments;
        if self.workspace_tabs.active() == ActiveWorkspaceTab::Settings {
            self.cancel_shortcut_recording(cx);
        }

        let was_welcome = self.workspace_tabs.browse_requests();
        self.sidebar_tab = sidebar_tab;
        if was_welcome {
            self.hide_preview(cx);
            self.restore_active_request_tab(window, cx);
        }
        if self.sidebar_tab == SidebarTab::Environments {
            self.hide_preview(cx);
        } else if !was_welcome
            && (was_tool || was_environment)
            && self.response_tab == ResponseTab::Preview
        {
            self.show_preview(window, cx);
        }
        cx.notify();
    }

    pub(super) fn activate_workspace_tab(
        &mut self,
        tab: WorkspaceTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match tab {
            WorkspaceTab::Welcome => {
                let was_settings = self.workspace_tabs.active() == ActiveWorkspaceTab::Settings;
                if self.workspace_tabs.activate_welcome() {
                    if was_settings {
                        self.cancel_shortcut_recording(cx);
                    }
                    self.hide_preview(cx);
                    cx.notify();
                }
            }
            WorkspaceTab::Request(tab_id) => self.activate_request_tab(tab_id, window, cx),
            WorkspaceTab::Tool(WorkspaceToolTab::Settings) => {
                self.open_workspace_tool_tab(WorkspaceToolTab::Settings, window, cx);
            }
            WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss) => {
                if self.theme_editor.is_some() {
                    self.open_workspace_tool_tab(WorkspaceToolTab::ThemeCss, window, cx);
                    if let Some(editor) = self.theme_editor.as_ref() {
                        editor.read(cx).focus_handle(cx).focus(window);
                    }
                } else {
                    self.open_theme_editor(window, cx);
                }
            }
        }
    }

    pub(super) fn close_workspace_tool_tab(
        &mut self,
        tab: WorkspaceToolTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let closed = match tab {
            WorkspaceToolTab::Settings => {
                self.cancel_shortcut_recording(cx);
                self.workspace_tabs
                    .close_tool(tab, self.theme_editor.is_some())
            }
            WorkspaceToolTab::ThemeCss => self.close_theme_editor(cx),
        };
        if !closed {
            return;
        }

        self.restore_visible_workspace_after_tool_close(window, cx);
        cx.notify();
    }

    pub(super) fn close_active_workspace_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.workspace_tabs.active() {
            ActiveWorkspaceTab::Welcome => {}
            ActiveWorkspaceTab::Request => {
                let tab_id = self.request_tabs.active_tab_id().clone();
                self.request_close_request_tab(tab_id, window, cx);
            }
            ActiveWorkspaceTab::Settings => {
                self.close_workspace_tool_tab(WorkspaceToolTab::Settings, window, cx);
            }
            ActiveWorkspaceTab::ThemeCss => {
                self.close_workspace_tool_tab(WorkspaceToolTab::ThemeCss, window, cx);
            }
        }
    }

    fn restore_visible_workspace_after_tool_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.workspace_tabs.active() {
            ActiveWorkspaceTab::Welcome => {
                self.hide_preview(cx);
            }
            ActiveWorkspaceTab::Request => {
                if self.sidebar_tab == SidebarTab::Environments {
                    self.hide_preview(cx);
                } else if self.response_tab == ResponseTab::Preview {
                    self.show_preview(window, cx);
                }
            }
            ActiveWorkspaceTab::Settings | ActiveWorkspaceTab::ThemeCss => {
                self.hide_preview(cx);
            }
        }
    }
}
