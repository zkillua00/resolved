use super::*;

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
        let tab = self.workspace_tabs.adjacent_tab(
            &self.request_tabs,
            self.theme_editor.is_some(),
            direction,
        );
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
        self.activate_request_workspace(SidebarTab::Collections, window, cx);
        self.request_pane = RequestPane::Body;
        self.format_raw_body(window, cx);
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
