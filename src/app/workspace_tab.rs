use crate::core::{RequestTabId, RequestTabs};

/// The content surface currently shown below the workspace tab strip.
///
/// Request records stay in [`RequestTabs`]. Settings and theme editing are
/// runtime-only singleton tools and never enter request persistence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ActiveWorkspaceTab {
    #[default]
    Request,
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
    Request(RequestTabId),
    Tool(WorkspaceToolTab),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct WorkspaceTabs {
    active: ActiveWorkspaceTab,
    settings_open: bool,
}

impl WorkspaceTabs {
    pub fn active(&self) -> ActiveWorkspaceTab {
        self.active
    }

    pub fn settings_open(&self) -> bool {
        self.settings_open
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
        self.active = ActiveWorkspaceTab::Request;
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
                    } else {
                        ActiveWorkspaceTab::Request
                    };
                }
            }
            WorkspaceToolTab::ThemeCss => {
                if self.active == ActiveWorkspaceTab::ThemeCss {
                    self.active = if self.settings_open {
                        ActiveWorkspaceTab::Settings
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
        let mut tabs = request_tabs
            .tabs()
            .iter()
            .map(|tab| WorkspaceTab::Request(tab.id().clone()))
            .collect::<Vec<_>>();
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
}
