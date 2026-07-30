use crate::core::{RequestTabId, RequestTabs};

/// The content surface currently shown below the workspace tab strip.
///
/// Request records stay in [`RequestTabs`]. Settings and theme editing are
/// runtime-only singleton tools and never enter request persistence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ActiveWorkspaceTab {
    #[default]
    Request,
    Welcome,
    Settings,
    ThemeCss,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkspaceToolTab {
    Settings,
    ThemeCss,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum WorkspaceTab {
    Welcome,
    Request(RequestTabId),
    Tool(WorkspaceToolTab),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct WorkspaceTabs {
    active: ActiveWorkspaceTab,
    settings_open: bool,
    welcome_request_tab_id: Option<RequestTabId>,
    welcome_visible: bool,
}

impl WorkspaceTabs {
    pub fn from_request_tabs(request_tabs: &RequestTabs) -> Self {
        let mut tabs = Self::default();
        if request_tabs.welcome_is_open() {
            tabs.show_welcome(request_tabs.active_tab_id().clone());
        }
        tabs
    }

    pub fn active(&self) -> ActiveWorkspaceTab {
        self.active
    }

    pub fn settings_open(&self) -> bool {
        self.settings_open
    }

    pub fn welcome_request_tab_id(&self) -> Option<&RequestTabId> {
        self.welcome_request_tab_id.as_ref()
    }

    pub fn welcome_is_open(&self) -> bool {
        self.welcome_visible
    }

    pub fn show_welcome(&mut self, backing_tab_id: RequestTabId) {
        self.open_welcome(backing_tab_id, true);
    }

    pub fn open_welcome(&mut self, backing_tab_id: RequestTabId, activate: bool) {
        self.welcome_request_tab_id = Some(backing_tab_id);
        self.welcome_visible = true;
        if activate {
            self.active = ActiveWorkspaceTab::Welcome;
        }
    }

    /// Remove the Welcome presentation and return its clean backing request.
    pub fn take_welcome_request_tab_id(&mut self) -> Option<RequestTabId> {
        let tab_id = self.welcome_request_tab_id.take();
        self.welcome_visible = false;
        if tab_id.is_some() && self.active == ActiveWorkspaceTab::Welcome {
            self.active = ActiveWorkspaceTab::Request;
        }
        tab_id
    }

    pub fn tool_is_active(&self, tab: WorkspaceToolTab) -> bool {
        self.active
            == match tab {
                WorkspaceToolTab::Settings => ActiveWorkspaceTab::Settings,
                WorkspaceToolTab::ThemeCss => ActiveWorkspaceTab::ThemeCss,
            }
    }

    pub fn open_tool(&mut self, tab: WorkspaceToolTab) {
        match tab {
            WorkspaceToolTab::Settings => {
                self.settings_open = true;
                self.active = ActiveWorkspaceTab::Settings;
            }
            WorkspaceToolTab::ThemeCss => {
                self.active = ActiveWorkspaceTab::ThemeCss;
            }
        }
    }

    pub fn activate_request(&mut self) {
        self.welcome_request_tab_id = None;
        self.welcome_visible = false;
        self.active = ActiveWorkspaceTab::Request;
    }

    /// Reveal the request workspace while retaining a pristine Welcome
    /// backing tab until the user edits it or opens a concrete request.
    pub fn browse_requests(&mut self) -> bool {
        let hid_welcome = self.welcome_visible;
        self.welcome_visible = false;
        self.active = ActiveWorkspaceTab::Request;
        hid_welcome
    }

    pub fn activate_welcome(&mut self) -> bool {
        if !self.welcome_visible {
            return false;
        }
        self.active = ActiveWorkspaceTab::Welcome;
        true
    }

    /// Close a singleton tool tab.
    ///
    /// Theme CSS openness is owned by the editor entity, so callers pass its
    /// post-operation state when closing Settings. Closing the active tool
    /// selects the next tool to its right, then the previous tool, then the
    /// retained active request.
    pub fn close_tool(&mut self, tab: WorkspaceToolTab, theme_css_open: bool) -> bool {
        match tab {
            WorkspaceToolTab::Settings => {
                if !self.settings_open {
                    return false;
                }
                self.settings_open = false;
                if self.active == ActiveWorkspaceTab::Settings {
                    self.active = if theme_css_open {
                        ActiveWorkspaceTab::ThemeCss
                    } else if self.welcome_visible {
                        ActiveWorkspaceTab::Welcome
                    } else {
                        ActiveWorkspaceTab::Request
                    };
                }
            }
            WorkspaceToolTab::ThemeCss => {
                if self.active == ActiveWorkspaceTab::ThemeCss {
                    self.active = if self.settings_open {
                        ActiveWorkspaceTab::Settings
                    } else if self.welcome_visible {
                        ActiveWorkspaceTab::Welcome
                    } else {
                        ActiveWorkspaceTab::Request
                    };
                }
            }
        }
        true
    }

    pub fn active_tab(&self, request_tabs: &RequestTabs) -> WorkspaceTab {
        match self.active {
            ActiveWorkspaceTab::Welcome => WorkspaceTab::Welcome,
            ActiveWorkspaceTab::Request => {
                WorkspaceTab::Request(request_tabs.active_tab_id().clone())
            }
            ActiveWorkspaceTab::Settings => WorkspaceTab::Tool(WorkspaceToolTab::Settings),
            ActiveWorkspaceTab::ThemeCss => WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss),
        }
    }

    pub fn visible_tabs(
        &self,
        request_tabs: &RequestTabs,
        theme_css_open: bool,
    ) -> Vec<WorkspaceTab> {
        let mut tabs = Vec::new();
        if self.welcome_visible {
            tabs.push(WorkspaceTab::Welcome);
        }
        tabs.extend(
            request_tabs
                .tabs()
                .iter()
                .filter(|tab| {
                    !self.welcome_visible || self.welcome_request_tab_id.as_ref() != Some(tab.id())
                })
                .map(|tab| WorkspaceTab::Request(tab.id().clone())),
        );
        if self.settings_open {
            tabs.push(WorkspaceTab::Tool(WorkspaceToolTab::Settings));
        }
        if theme_css_open {
            tabs.push(WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss));
        }
        tabs
    }

    pub fn adjacent_tab(
        &self,
        request_tabs: &RequestTabs,
        theme_css_open: bool,
        direction: isize,
    ) -> WorkspaceTab {
        let tabs = self.visible_tabs(request_tabs, theme_css_open);
        let active = self.active_tab(request_tabs);
        let current = tabs.iter().position(|tab| tab == &active).unwrap_or(0);
        let next = (current as isize + direction).rem_euclid(tabs.len() as isize) as usize;
        tabs[next].clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requests_with_welcome() -> RequestTabs {
        let mut requests = RequestTabs::default();
        let initial = requests.active_tab_id().clone();
        requests.close(&initial).unwrap();
        requests
    }

    #[test]
    fn singleton_tools_have_stable_order_and_open_idempotently() {
        let requests = RequestTabs::default();
        let mut tabs = WorkspaceTabs::default();

        tabs.open_tool(WorkspaceToolTab::Settings);
        tabs.open_tool(WorkspaceToolTab::Settings);
        assert_eq!(tabs.active(), ActiveWorkspaceTab::Settings);
        assert_eq!(
            tabs.visible_tabs(&requests, false),
            vec![
                WorkspaceTab::Request(requests.active_tab_id().clone()),
                WorkspaceTab::Tool(WorkspaceToolTab::Settings),
            ]
        );

        tabs.open_tool(WorkspaceToolTab::ThemeCss);
        assert_eq!(tabs.active(), ActiveWorkspaceTab::ThemeCss);
        assert_eq!(
            tabs.visible_tabs(&requests, true),
            vec![
                WorkspaceTab::Request(requests.active_tab_id().clone()),
                WorkspaceTab::Tool(WorkspaceToolTab::Settings),
                WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss),
            ]
        );
    }

    #[test]
    fn welcome_replaces_only_its_backing_scratch_and_is_idempotent() {
        let requests = requests_with_welcome();
        let backing_id = requests.active_tab_id().clone();
        let mut tabs = WorkspaceTabs::from_request_tabs(&requests);

        assert_eq!(tabs.active(), ActiveWorkspaceTab::Welcome);
        assert_eq!(
            tabs.visible_tabs(&requests, false),
            vec![WorkspaceTab::Welcome]
        );
        assert!(tabs.activate_welcome());
        assert_eq!(tabs.take_welcome_request_tab_id(), Some(backing_id.clone()));
        assert_eq!(tabs.active(), ActiveWorkspaceTab::Request);
        assert_eq!(
            tabs.visible_tabs(&requests, false),
            vec![WorkspaceTab::Request(backing_id)]
        );
    }

    #[test]
    fn a_legacy_pristine_request_does_not_implicitly_become_welcome() {
        let requests = RequestTabs::default();
        let tabs = WorkspaceTabs::from_request_tabs(&requests);

        assert_eq!(tabs.active(), ActiveWorkspaceTab::Request);
        assert!(!tabs.welcome_is_open());
        assert_eq!(
            tabs.visible_tabs(&requests, false),
            vec![WorkspaceTab::Request(requests.active_tab_id().clone())]
        );
    }

    #[test]
    fn browsing_requests_hides_welcome_without_consuming_its_backing_tab() {
        let requests = requests_with_welcome();
        let backing_id = requests.active_tab_id().clone();
        let mut tabs = WorkspaceTabs::from_request_tabs(&requests);

        assert!(tabs.browse_requests());
        assert_eq!(tabs.active(), ActiveWorkspaceTab::Request);
        assert!(!tabs.welcome_is_open());
        assert_eq!(tabs.welcome_request_tab_id(), Some(&backing_id));
        assert_eq!(
            tabs.visible_tabs(&requests, false),
            vec![WorkspaceTab::Request(backing_id)]
        );
    }

    #[test]
    fn closing_active_tools_uses_visual_neighbor_fallbacks() {
        let mut tabs = WorkspaceTabs::default();
        tabs.open_tool(WorkspaceToolTab::Settings);
        tabs.open_tool(WorkspaceToolTab::ThemeCss);

        assert!(tabs.close_tool(WorkspaceToolTab::ThemeCss, false));
        assert_eq!(tabs.active(), ActiveWorkspaceTab::Settings);
        assert!(tabs.close_tool(WorkspaceToolTab::Settings, false));
        assert_eq!(tabs.active(), ActiveWorkspaceTab::Request);
        assert!(!tabs.close_tool(WorkspaceToolTab::Settings, false));
    }

    #[test]
    fn closing_settings_can_fall_forward_to_theme_css() {
        let mut tabs = WorkspaceTabs::default();
        tabs.open_tool(WorkspaceToolTab::ThemeCss);
        tabs.open_tool(WorkspaceToolTab::Settings);

        assert!(tabs.close_tool(WorkspaceToolTab::Settings, true));
        assert_eq!(tabs.active(), ActiveWorkspaceTab::ThemeCss);
    }

    #[test]
    fn adjacent_navigation_wraps_across_requests_and_tools() {
        let mut requests = RequestTabs::default();
        let first = requests.active_tab_id().clone();
        let second = requests.open_new();
        let mut tabs = WorkspaceTabs::default();
        tabs.open_tool(WorkspaceToolTab::Settings);

        assert_eq!(
            tabs.adjacent_tab(&requests, false, -1),
            WorkspaceTab::Request(second.clone())
        );
        tabs.activate_request();
        assert_eq!(
            tabs.adjacent_tab(&requests, false, 1),
            WorkspaceTab::Tool(WorkspaceToolTab::Settings)
        );
        let _ = requests.activate(&first);
        assert_eq!(
            tabs.adjacent_tab(&requests, false, -1),
            WorkspaceTab::Tool(WorkspaceToolTab::Settings)
        );
    }

    #[test]
    fn tools_fall_back_to_welcome_and_adjacent_navigation_includes_it_once() {
        let requests = requests_with_welcome();
        let mut tabs = WorkspaceTabs::from_request_tabs(&requests);
        tabs.open_tool(WorkspaceToolTab::Settings);

        assert_eq!(
            tabs.visible_tabs(&requests, false),
            vec![
                WorkspaceTab::Welcome,
                WorkspaceTab::Tool(WorkspaceToolTab::Settings),
            ]
        );
        assert_eq!(
            tabs.adjacent_tab(&requests, false, -1),
            WorkspaceTab::Welcome
        );
        assert!(tabs.close_tool(WorkspaceToolTab::Settings, false));
        assert_eq!(tabs.active(), ActiveWorkspaceTab::Welcome);
    }

    #[test]
    fn opening_welcome_behind_an_active_tool_does_not_steal_focus() {
        let requests = RequestTabs::default();
        let mut tabs = WorkspaceTabs::default();
        tabs.open_tool(WorkspaceToolTab::Settings);
        tabs.open_welcome(requests.active_tab_id().clone(), false);

        assert_eq!(tabs.active(), ActiveWorkspaceTab::Settings);
        assert_eq!(
            tabs.visible_tabs(&requests, false),
            vec![
                WorkspaceTab::Welcome,
                WorkspaceTab::Tool(WorkspaceToolTab::Settings),
            ]
        );
        assert!(tabs.close_tool(WorkspaceToolTab::Settings, false));
        assert_eq!(tabs.active(), ActiveWorkspaceTab::Welcome);
    }
}
