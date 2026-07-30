use crate::core::{RequestTabId, RequestTabs};

/// The content surface currently shown below the workspace tab strip.
///
/// Request records stay in [`RequestTabs`]. Settings and theme editors are
/// runtime-only tools and never enter request persistence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ActiveWorkspaceTab {
    #[default]
    Request,
    Welcome,
    Settings,
    ThemeCss,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum WorkspaceToolTab {
    Settings,
    ThemeCss(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum WorkspaceTab {
    Welcome,
    Request(RequestTabId),
    Tool(WorkspaceToolTab),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkspaceTabCloseScope {
    Current,
    Others,
    ToLeft,
    ToRight,
    All,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct WorkspaceTabs {
    active: ActiveWorkspaceTab,
    settings_open: bool,
    theme_editor_ids: Vec<String>,
    active_theme_editor_id: Option<String>,
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

    pub fn theme_editor_ids(&self) -> &[String] {
        &self.theme_editor_ids
    }

    pub fn active_theme_editor_id(&self) -> Option<&str> {
        (self.active == ActiveWorkspaceTab::ThemeCss)
            .then_some(self.active_theme_editor_id.as_deref())
            .flatten()
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

    pub fn tool_is_active(&self, tab: &WorkspaceToolTab) -> bool {
        match tab {
            WorkspaceToolTab::Settings => self.active == ActiveWorkspaceTab::Settings,
            WorkspaceToolTab::ThemeCss(editor_id) => {
                self.active == ActiveWorkspaceTab::ThemeCss
                    && self.active_theme_editor_id.as_ref() == Some(editor_id)
            }
        }
    }

    pub fn open_tool(&mut self, tab: WorkspaceToolTab) {
        match tab {
            WorkspaceToolTab::Settings => {
                self.settings_open = true;
                self.active = ActiveWorkspaceTab::Settings;
            }
            WorkspaceToolTab::ThemeCss(editor_id) => {
                if !self
                    .theme_editor_ids
                    .iter()
                    .any(|candidate| candidate == &editor_id)
                {
                    self.theme_editor_ids.push(editor_id.clone());
                }
                self.active_theme_editor_id = Some(editor_id);
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

    /// Close a runtime-only tool tab. Closing the active tool selects the next
    /// tab to its right, then the previous tool, then the retained request.
    pub fn close_tool(&mut self, tab: &WorkspaceToolTab) -> bool {
        match tab {
            WorkspaceToolTab::Settings => {
                if !self.settings_open {
                    return false;
                }
                self.settings_open = false;
                if self.active == ActiveWorkspaceTab::Settings {
                    self.active = if !self.theme_editor_ids.is_empty() {
                        self.active_theme_editor_id = self.theme_editor_ids.first().cloned();
                        ActiveWorkspaceTab::ThemeCss
                    } else if self.welcome_visible {
                        ActiveWorkspaceTab::Welcome
                    } else {
                        ActiveWorkspaceTab::Request
                    };
                }
            }
            WorkspaceToolTab::ThemeCss(editor_id) => {
                let Some(index) = self
                    .theme_editor_ids
                    .iter()
                    .position(|candidate| candidate == editor_id)
                else {
                    return false;
                };
                let was_active = self.active == ActiveWorkspaceTab::ThemeCss
                    && self.active_theme_editor_id.as_ref() == Some(editor_id);
                self.theme_editor_ids.remove(index);
                if was_active {
                    self.active = if !self.theme_editor_ids.is_empty() {
                        self.active_theme_editor_id = self
                            .theme_editor_ids
                            .get(index)
                            .or_else(|| self.theme_editor_ids.last())
                            .cloned();
                        ActiveWorkspaceTab::ThemeCss
                    } else if self.settings_open {
                        self.active_theme_editor_id = None;
                        ActiveWorkspaceTab::Settings
                    } else if self.welcome_visible {
                        self.active_theme_editor_id = None;
                        ActiveWorkspaceTab::Welcome
                    } else {
                        self.active_theme_editor_id = None;
                        ActiveWorkspaceTab::Request
                    };
                }
            }
        }
        true
    }

    pub fn replace_theme_editor_id(&mut self, old_id: &str, new_id: String) -> bool {
        let Some(mut index) = self
            .theme_editor_ids
            .iter()
            .position(|candidate| candidate == old_id)
        else {
            return false;
        };
        if let Some(existing) = self
            .theme_editor_ids
            .iter()
            .position(|candidate| candidate == &new_id)
            && existing != index
        {
            self.theme_editor_ids.remove(existing);
            if existing < index {
                index -= 1;
            }
        }
        if self.active_theme_editor_id.as_deref() == Some(old_id) {
            self.active_theme_editor_id = Some(new_id.clone());
        }
        self.theme_editor_ids[index] = new_id;
        true
    }

    pub fn active_tab(&self, request_tabs: &RequestTabs) -> WorkspaceTab {
        match self.active {
            ActiveWorkspaceTab::Welcome => WorkspaceTab::Welcome,
            ActiveWorkspaceTab::Request => {
                WorkspaceTab::Request(request_tabs.active_tab_id().clone())
            }
            ActiveWorkspaceTab::Settings => WorkspaceTab::Tool(WorkspaceToolTab::Settings),
            ActiveWorkspaceTab::ThemeCss => WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss(
                self.active_theme_editor_id
                    .as_ref()
                    .expect("active theme editor must have a tab identity")
                    .clone(),
            )),
        }
    }

    pub fn visible_tabs(&self, request_tabs: &RequestTabs) -> Vec<WorkspaceTab> {
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
        tabs.extend(
            self.theme_editor_ids
                .iter()
                .cloned()
                .map(|editor_id| WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss(editor_id))),
        );
        tabs
    }

    pub fn adjacent_tab(&self, request_tabs: &RequestTabs, direction: isize) -> WorkspaceTab {
        let tabs = self.visible_tabs(request_tabs);
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
    fn tools_have_stable_order_and_open_idempotently() {
        let requests = RequestTabs::default();
        let mut tabs = WorkspaceTabs::default();

        tabs.open_tool(WorkspaceToolTab::Settings);
        tabs.open_tool(WorkspaceToolTab::Settings);
        assert_eq!(tabs.active(), ActiveWorkspaceTab::Settings);
        assert_eq!(
            tabs.visible_tabs(&requests),
            vec![
                WorkspaceTab::Request(requests.active_tab_id().clone()),
                WorkspaceTab::Tool(WorkspaceToolTab::Settings),
            ]
        );

        tabs.open_tool(WorkspaceToolTab::ThemeCss("ocean".to_owned()));
        tabs.open_tool(WorkspaceToolTab::ThemeCss("forest".to_owned()));
        assert_eq!(tabs.active(), ActiveWorkspaceTab::ThemeCss);
        assert_eq!(
            tabs.visible_tabs(&requests),
            vec![
                WorkspaceTab::Request(requests.active_tab_id().clone()),
                WorkspaceTab::Tool(WorkspaceToolTab::Settings),
                WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss("ocean".to_owned())),
                WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss("forest".to_owned())),
            ]
        );
        assert!(tabs.tool_is_active(&WorkspaceToolTab::ThemeCss("forest".to_owned())));
        tabs.open_tool(WorkspaceToolTab::ThemeCss("ocean".to_owned()));
        assert_eq!(tabs.theme_editor_ids(), &["ocean", "forest"]);
        assert!(tabs.tool_is_active(&WorkspaceToolTab::ThemeCss("ocean".to_owned())));
    }

    #[test]
    fn welcome_replaces_only_its_backing_scratch_and_is_idempotent() {
        let requests = requests_with_welcome();
        let backing_id = requests.active_tab_id().clone();
        let mut tabs = WorkspaceTabs::from_request_tabs(&requests);

        assert_eq!(tabs.active(), ActiveWorkspaceTab::Welcome);
        assert_eq!(tabs.visible_tabs(&requests), vec![WorkspaceTab::Welcome]);
        assert!(tabs.activate_welcome());
        assert_eq!(tabs.take_welcome_request_tab_id(), Some(backing_id.clone()));
        assert_eq!(tabs.active(), ActiveWorkspaceTab::Request);
        assert_eq!(
            tabs.visible_tabs(&requests),
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
            tabs.visible_tabs(&requests),
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
            tabs.visible_tabs(&requests),
            vec![WorkspaceTab::Request(backing_id)]
        );
    }

    #[test]
    fn closing_active_tools_uses_visual_neighbor_fallbacks() {
        let mut tabs = WorkspaceTabs::default();
        tabs.open_tool(WorkspaceToolTab::Settings);
        tabs.open_tool(WorkspaceToolTab::ThemeCss("ocean".to_owned()));
        tabs.open_tool(WorkspaceToolTab::ThemeCss("forest".to_owned()));
        tabs.open_tool(WorkspaceToolTab::ThemeCss("ocean".to_owned()));

        assert!(tabs.close_tool(&WorkspaceToolTab::ThemeCss("ocean".to_owned())));
        assert!(tabs.tool_is_active(&WorkspaceToolTab::ThemeCss("forest".to_owned())));
        assert!(tabs.close_tool(&WorkspaceToolTab::ThemeCss("forest".to_owned())));
        assert_eq!(tabs.active(), ActiveWorkspaceTab::Settings);
        assert!(tabs.close_tool(&WorkspaceToolTab::Settings));
        assert_eq!(tabs.active(), ActiveWorkspaceTab::Request);
        assert!(!tabs.close_tool(&WorkspaceToolTab::Settings));
    }

    #[test]
    fn closing_settings_can_fall_forward_to_theme_css() {
        let mut tabs = WorkspaceTabs::default();
        tabs.open_tool(WorkspaceToolTab::ThemeCss("ocean".to_owned()));
        tabs.open_tool(WorkspaceToolTab::Settings);

        assert!(tabs.close_tool(&WorkspaceToolTab::Settings));
        assert_eq!(tabs.active(), ActiveWorkspaceTab::ThemeCss);
        assert!(tabs.tool_is_active(&WorkspaceToolTab::ThemeCss("ocean".to_owned())));
    }

    #[test]
    fn adjacent_navigation_wraps_across_requests_and_tools() {
        let mut requests = RequestTabs::default();
        let first = requests.active_tab_id().clone();
        let second = requests.open_new();
        let mut tabs = WorkspaceTabs::default();
        tabs.open_tool(WorkspaceToolTab::Settings);

        assert_eq!(
            tabs.adjacent_tab(&requests, -1),
            WorkspaceTab::Request(second.clone())
        );
        tabs.activate_request();
        assert_eq!(
            tabs.adjacent_tab(&requests, 1),
            WorkspaceTab::Tool(WorkspaceToolTab::Settings)
        );
        let _ = requests.activate(&first);
        assert_eq!(
            tabs.adjacent_tab(&requests, -1),
            WorkspaceTab::Tool(WorkspaceToolTab::Settings)
        );
    }

    #[test]
    fn tools_fall_back_to_welcome_and_adjacent_navigation_includes_it_once() {
        let requests = requests_with_welcome();
        let mut tabs = WorkspaceTabs::from_request_tabs(&requests);
        tabs.open_tool(WorkspaceToolTab::Settings);

        assert_eq!(
            tabs.visible_tabs(&requests),
            vec![
                WorkspaceTab::Welcome,
                WorkspaceTab::Tool(WorkspaceToolTab::Settings),
            ]
        );
        assert_eq!(tabs.adjacent_tab(&requests, -1), WorkspaceTab::Welcome);
        assert!(tabs.close_tool(&WorkspaceToolTab::Settings));
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
            tabs.visible_tabs(&requests),
            vec![
                WorkspaceTab::Welcome,
                WorkspaceTab::Tool(WorkspaceToolTab::Settings),
            ]
        );
        assert!(tabs.close_tool(&WorkspaceToolTab::Settings));
        assert_eq!(tabs.active(), ActiveWorkspaceTab::Welcome);
    }
}
