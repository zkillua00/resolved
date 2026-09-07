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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(tab, WorkspaceToolTab::ServerTools)
            && !matches!(
                self.workspace_providers.active_id(),
                WorkspaceProviderId::Upstream { .. }
            )
        {
            return;
        }
        self.dismiss_template_variable_popover();
        if self.workspace_tabs.active() == ActiveWorkspaceTab::Request {
            self.snapshot_active_request_tab(cx);
            self.hide_preview(cx);
        } else if self.workspace_tabs.active() == ActiveWorkspaceTab::Settings
            && !matches!(tab, WorkspaceToolTab::Settings)
        {
            self.cancel_shortcut_recording(cx);
        }

        let opening_server_management = matches!(
            tab,
            WorkspaceToolTab::RequestProxy | WorkspaceToolTab::ServerTools
        );
        self.workspace_tabs.open_tool(tab);
        if opening_server_management {
            self.ensure_server_management_loaded(window, cx);
        }
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
        self.navigation_sidebar_open = sidebar_tab != SidebarTab::Environments;
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

    pub(super) fn toggle_navigation_sidebar(
        &mut self,
        sidebar_tab: SidebarTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace_tabs.active() == ActiveWorkspaceTab::Request
            && self.sidebar_tab == sidebar_tab
            && self.navigation_sidebar_open
        {
            self.navigation_sidebar_open = false;
            cx.notify();
        } else {
            self.activate_request_workspace(sidebar_tab, window, cx);
        }
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
            WorkspaceTab::Tool(tool) => match tool {
                WorkspaceToolTab::Snippets => {
                    self.open_workspace_tool_tab(WorkspaceToolTab::Snippets, window, cx);
                }
                WorkspaceToolTab::RequestProxy => {
                    self.open_workspace_tool_tab(WorkspaceToolTab::RequestProxy, window, cx);
                }
                WorkspaceToolTab::ServerTools => {
                    self.open_workspace_tool_tab(WorkspaceToolTab::ServerTools, window, cx);
                }
                WorkspaceToolTab::Settings => {
                    self.open_workspace_tool_tab(WorkspaceToolTab::Settings, window, cx);
                }
                WorkspaceToolTab::ThemeCss(editor_id) => {
                    if let Some(editor) = self
                        .theme_editors
                        .get(&editor_id)
                        .map(|session| session.editor.clone())
                    {
                        self.open_workspace_tool_tab(
                            WorkspaceToolTab::ThemeCss(editor_id),
                            window,
                            cx,
                        );
                        editor.read(cx).focus_handle(cx).focus(window);
                    }
                }
            },
        }
    }

    /// Activate a tab in the strip that owns it. Secondary panes keep their
    /// selection local; only the pane hosting the global request surface uses
    /// the legacy global activation path.
    pub(super) fn activate_workspace_tab_in_pane(
        &mut self,
        tab: WorkspaceTab,
        pane_id: Option<PaneId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane_id) = pane_id else {
            self.activate_workspace_tab(tab, window, cx);
            return;
        };
        let global_tab = self.workspace_tabs.active_tab(&self.request_tabs);
        let primary_pane_id = self.panes.pane_for_tab(&global_tab);
        if primary_pane_id == Some(pane_id) {
            self.activate_workspace_tab(tab, window, cx);
            return;
        }
        if !self
            .panes
            .pane(pane_id)
            .is_some_and(|pane| pane.contains(&tab))
        {
            return;
        }
        let changed = self
            .panes
            .pane_mut(pane_id)
            .is_some_and(|pane| pane.activate(&tab));
        if let WorkspaceTab::Request(tab_id) = tab {
            self.ensure_pane_editor_for(pane_id, tab_id, window, cx);
        }
        if changed {
            cx.notify();
        }
    }

    pub(super) fn close_workspace_tool_tab(
        &mut self,
        tab: WorkspaceToolTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let closing = WorkspaceTab::Tool(tab.clone());
        let active_will_close = self.workspace_tabs.active_tab(&self.request_tabs) == closing;
        let fallback = self
            .workspace_tabs
            .fallback_after_closing(&self.request_tabs, std::slice::from_ref(&closing));
        let closed = match &tab {
            WorkspaceToolTab::Snippets => self.workspace_tabs.close_tool(&tab),
            WorkspaceToolTab::RequestProxy => self.workspace_tabs.close_tool(&tab),
            WorkspaceToolTab::ServerTools => self.workspace_tabs.close_tool(&tab),
            WorkspaceToolTab::Settings => {
                self.cancel_shortcut_recording(cx);
                self.workspace_tabs.close_tool(&tab)
            }
            WorkspaceToolTab::ThemeCss(editor_id) => self.close_theme_editor(editor_id, cx),
        };
        if !closed {
            return;
        }

        if active_will_close && let Some(fallback) = fallback {
            self.activate_workspace_tab(fallback, window, cx);
            return;
        }
        self.restore_visible_workspace_after_tool_close(window, cx);
        cx.notify();
    }

    pub(super) fn request_close_workspace_tabs(
        &mut self,
        anchor: WorkspaceTab,
        scope: WorkspaceTabCloseScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self
            .workspace_tabs
            .close_targets(&self.request_tabs, &anchor, scope);
        if targets.is_empty() {
            return;
        }
        let request_anchor_id = match anchor {
            WorkspaceTab::Request(tab_id) => tab_id,
            WorkspaceTab::Welcome | WorkspaceTab::Tool(_) => {
                self.request_tabs.active_tab_id().clone()
            }
        };

        let request_ids = targets
            .iter()
            .filter_map(|tab| match tab {
                WorkspaceTab::Request(tab_id) => Some(tab_id.clone()),
                WorkspaceTab::Welcome | WorkspaceTab::Tool(_) => None,
            })
            .collect::<Vec<_>>();
        let tools = targets
            .iter()
            .filter_map(|tab| match tab {
                WorkspaceTab::Tool(tool) => Some(tool.clone()),
                WorkspaceTab::Welcome | WorkspaceTab::Request(_) => None,
            })
            .collect::<Vec<_>>();

        if self.sending && !request_ids.is_empty() {
            self.request_notice =
                Some("Finish or cancel the active request before closing tabs.".to_owned());
            cx.notify();
            return;
        }
        if !request_ids.is_empty() {
            self.snapshot_active_request_tab(cx);
        }
        let dirty_titles = request_ids
            .iter()
            .filter_map(|id| self.request_tabs.get(id))
            .filter(|tab| tab.is_dirty())
            .map(|tab| tab.display_title().to_owned())
            .collect::<Vec<_>>();
        if dirty_titles.is_empty() {
            self.close_workspace_tabs_now(request_ids, tools, request_anchor_id, window, cx);
            return;
        }

        let close_count = targets.len();
        let dirty_count = dirty_titles.len();
        let dialog_title = if close_count == 1 {
            "Discard request changes?".to_owned()
        } else {
            format!("Close {close_count} tabs?")
        };
        let detail =
            super::request_tabs_actions::close_tabs_confirmation_detail(&dirty_titles, close_count);
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let close_this = this.clone();
            let request_ids = request_ids.clone();
            let tools = tools.clone();
            let request_anchor_id = request_anchor_id.clone();
            dialog
                .title(dialog_title.clone())
                .w(px(440.))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(if close_count == 1 {
                            "Discard".to_owned()
                        } else {
                            format!("Close {close_count} tabs")
                        })
                        .ok_variant(ButtonVariant::Danger),
                )
                .on_ok(move |_, window, cx| {
                    if let Some(this) = close_this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.close_workspace_tabs_now(
                                request_ids.clone(),
                                tools.clone(),
                                request_anchor_id.clone(),
                                window,
                                cx,
                            );
                        });
                    }
                    true
                })
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(detail.clone()),
                        )
                        .when(dirty_count > 1, |this| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!(
                                        "{dirty_count} tabs contain unsaved request changes."
                                    )),
                            )
                        }),
                )
        });
    }

    fn close_workspace_tabs_now(
        &mut self,
        request_ids: Vec<RequestTabId>,
        tools: Vec<WorkspaceToolTab>,
        request_anchor_id: RequestTabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut closing = request_ids
            .iter()
            .cloned()
            .map(WorkspaceTab::Request)
            .collect::<Vec<_>>();
        closing.extend(tools.iter().cloned().map(WorkspaceTab::Tool));
        let active_before = self.workspace_tabs.active_tab(&self.request_tabs);
        let active_will_close = closing.contains(&active_before);
        let fallback = self
            .workspace_tabs
            .fallback_after_closing(&self.request_tabs, &closing);

        for tool in tools.into_iter().rev() {
            let closed = match &tool {
                WorkspaceToolTab::Snippets => self.workspace_tabs.close_tool(&tool),
                WorkspaceToolTab::RequestProxy => self.workspace_tabs.close_tool(&tool),
                WorkspaceToolTab::ServerTools => self.workspace_tabs.close_tool(&tool),
                WorkspaceToolTab::Settings => {
                    self.cancel_shortcut_recording(cx);
                    self.workspace_tabs.close_tool(&tool)
                }
                WorkspaceToolTab::ThemeCss(editor_id) => self.close_theme_editor(editor_id, cx),
            };
            if !closed {
                return;
            }
        }

        if request_ids.is_empty() {
            if active_will_close && let Some(fallback) = fallback {
                self.activate_workspace_tab(fallback, window, cx);
                return;
            }
            self.restore_visible_workspace_after_tool_close(window, cx);
            cx.notify();
            return;
        }
        self.close_request_tabs_now(request_ids, request_anchor_id, window, cx);
        if active_will_close && let Some(fallback) = fallback {
            self.activate_workspace_tab(fallback, window, cx);
        }
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
            ActiveWorkspaceTab::Snippets => {
                self.close_workspace_tool_tab(WorkspaceToolTab::Snippets, window, cx);
            }
            ActiveWorkspaceTab::RequestProxy => {
                self.close_workspace_tool_tab(WorkspaceToolTab::RequestProxy, window, cx);
            }
            ActiveWorkspaceTab::ServerTools => {
                self.close_workspace_tool_tab(WorkspaceToolTab::ServerTools, window, cx);
            }
            ActiveWorkspaceTab::Settings => {
                self.close_workspace_tool_tab(WorkspaceToolTab::Settings, window, cx);
            }
            ActiveWorkspaceTab::ThemeCss => {
                if let Some(editor_id) = self
                    .workspace_tabs
                    .active_theme_editor_id()
                    .map(ToOwned::to_owned)
                {
                    self.close_workspace_tool_tab(
                        WorkspaceToolTab::ThemeCss(editor_id),
                        window,
                        cx,
                    );
                }
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
            ActiveWorkspaceTab::Snippets
            | ActiveWorkspaceTab::RequestProxy
            | ActiveWorkspaceTab::ServerTools
            | ActiveWorkspaceTab::Settings
            | ActiveWorkspaceTab::ThemeCss => {
                self.hide_preview(cx);
            }
        }
    }
}
