use super::*;

impl ApiTester {
    pub(crate) fn on_new_request_tab(
        &mut self,
        _: &shortcuts::NewRequestTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_tab = SidebarTab::Collections;
        self.open_blank_request_tab(window, cx);
    }

    pub(crate) fn on_close_request_tab(
        &mut self,
        _: &shortcuts::CloseRequestTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_tab = SidebarTab::Collections;
        let tab_id = self.request_tabs.active_tab_id().clone();
        self.request_close_request_tab(tab_id, window, cx);
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
        let tabs = self.request_tabs.tabs();
        if tabs.len() < 2 || self.sending {
            return;
        }
        let current = tabs
            .iter()
            .position(|tab| tab.id() == self.request_tabs.active_tab_id())
            .unwrap_or(0);
        let next = (current as isize + direction).rem_euclid(tabs.len() as isize) as usize;
        let tab_id = tabs[next].id().clone();
        self.sidebar_tab = SidebarTab::Collections;
        self.activate_request_tab(tab_id, window, cx);
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
            self.sidebar_tab = SidebarTab::Collections;
            self.start_request(window, cx);
        }
    }

    pub(crate) fn on_save_request(
        &mut self,
        _: &shortcuts::SaveRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_tab = SidebarTab::Collections;
        self.save_current_request(false, window, cx);
    }

    pub(crate) fn on_save_request_as(
        &mut self,
        _: &shortcuts::SaveRequestAs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_tab = SidebarTab::Collections;
        self.save_current_request(true, window, cx);
    }

    pub(crate) fn on_focus_request_url(
        &mut self,
        _: &shortcuts::FocusRequestUrl,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_tab = SidebarTab::Collections;
        self.url.read(cx).focus_handle(cx).focus(window);
        cx.notify();
    }

    pub(crate) fn on_format_raw_body(
        &mut self,
        _: &shortcuts::FormatRawBody,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_tab = SidebarTab::Collections;
        self.request_pane = RequestPane::Body;
        self.format_raw_body(window, cx);
    }

    pub(crate) fn on_show_collections(
        &mut self,
        _: &shortcuts::ShowCollections,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_tab = SidebarTab::Collections;
        cx.notify();
    }

    pub(crate) fn on_show_environments(
        &mut self,
        _: &shortcuts::ShowEnvironments,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_tab = SidebarTab::Environments;
        cx.notify();
    }

    pub(crate) fn on_show_history(
        &mut self,
        _: &shortcuts::ShowHistory,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_tab = SidebarTab::History;
        cx.notify();
    }

    pub(crate) fn on_show_settings(
        &mut self,
        _: &shortcuts::ShowSettings,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_tab = SidebarTab::Settings;
        cx.notify();
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
