use crate::core::{RequestTabId, RequestTabs};

/// The content surface currently shown below the workspace tab strip.
///
/// Request records stay in [`RequestTabs`]. Snippets, Request Proxy, Settings,
/// and theme editors are runtime-only tool surfaces and never enter
/// request-tab persistence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ActiveWorkspaceTab {
    #[default]
    Request,
    Welcome,
    Snippets,
    RequestProxy,
    Settings,
    ThemeCss,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum WorkspaceToolTab {
    Snippets,
    RequestProxy,
    Settings,
    ThemeCss(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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
    /// User-defined visual order for the shared workspace tab strip.
    ///
    /// Request records and runtime-only tools keep their own content state,
    /// but their controls are ordered together. `None` preserves the legacy
    /// default order until the first explicit reorder.
    tab_order: Option<Vec<WorkspaceTab>>,
    snippets_open: bool,
    request_proxy_open: bool,
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

    pub fn snippets_open(&self) -> bool {
        self.snippets_open
    }

    pub fn request_proxy_open(&self) -> bool {
        self.request_proxy_open
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
        self.replace_ordered_tab(
            &WorkspaceTab::Request(backing_tab_id.clone()),
            WorkspaceTab::Welcome,
        );
        self.welcome_request_tab_id = Some(backing_tab_id);
        self.welcome_visible = true;
        if activate {
            self.active = ActiveWorkspaceTab::Welcome;
        }
    }

    /// Remove the Welcome presentation and return its clean backing request.
    pub fn take_welcome_request_tab_id(&mut self) -> Option<RequestTabId> {
        let tab_id = self.welcome_request_tab_id.take();
        if let Some(tab_id) = tab_id.as_ref() {
            self.replace_ordered_tab(
                &WorkspaceTab::Welcome,
                WorkspaceTab::Request(tab_id.clone()),
            );
        }
        self.welcome_visible = false;
        if tab_id.is_some() && self.active == ActiveWorkspaceTab::Welcome {
            self.active = ActiveWorkspaceTab::Request;
        }
        tab_id
    }

    #[cfg(test)]
    pub fn tool_is_active(&self, tab: &WorkspaceToolTab) -> bool {
        match tab {
            WorkspaceToolTab::Snippets => self.active == ActiveWorkspaceTab::Snippets,
            WorkspaceToolTab::RequestProxy => self.active == ActiveWorkspaceTab::RequestProxy,
            WorkspaceToolTab::Settings => self.active == ActiveWorkspaceTab::Settings,
            WorkspaceToolTab::ThemeCss(editor_id) => {
                self.active == ActiveWorkspaceTab::ThemeCss
                    && self.active_theme_editor_id.as_ref() == Some(editor_id)
            }
        }
    }

    pub fn open_tool(&mut self, tab: WorkspaceToolTab) {
        match tab {
            WorkspaceToolTab::Snippets => {
                self.snippets_open = true;
                self.active = ActiveWorkspaceTab::Snippets;
            }
            WorkspaceToolTab::RequestProxy => {
                self.request_proxy_open = true;
                self.active = ActiveWorkspaceTab::RequestProxy;
            }
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
        if let Some(tab_id) = self.welcome_request_tab_id.clone() {
            self.replace_ordered_tab(&WorkspaceTab::Welcome, WorkspaceTab::Request(tab_id));
        }
        self.welcome_request_tab_id = None;
        self.welcome_visible = false;
        self.active = ActiveWorkspaceTab::Request;
    }

    /// Reveal the request workspace while retaining a pristine Welcome
    /// backing tab until the user edits it or opens a concrete request.
    pub fn browse_requests(&mut self) -> bool {
        let hid_welcome = self.welcome_visible;
        if let Some(tab_id) = self.welcome_request_tab_id.clone() {
            self.replace_ordered_tab(&WorkspaceTab::Welcome, WorkspaceTab::Request(tab_id));
        }
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
            WorkspaceToolTab::Snippets => {
                if !self.snippets_open {
                    return false;
                }
                self.snippets_open = false;
                if self.active == ActiveWorkspaceTab::Snippets {
                    self.active = if self.request_proxy_open {
                        ActiveWorkspaceTab::RequestProxy
                    } else if self.settings_open {
                        ActiveWorkspaceTab::Settings
                    } else if !self.theme_editor_ids.is_empty() {
                        self.active_theme_editor_id = self.theme_editor_ids.first().cloned();
                        ActiveWorkspaceTab::ThemeCss
                    } else if self.welcome_visible {
                        ActiveWorkspaceTab::Welcome
                    } else {
                        ActiveWorkspaceTab::Request
                    };
                }
            }
            WorkspaceToolTab::RequestProxy => {
                if !self.request_proxy_open {
                    return false;
                }
                self.request_proxy_open = false;
                if self.active == ActiveWorkspaceTab::RequestProxy {
                    self.active = if self.settings_open {
                        ActiveWorkspaceTab::Settings
                    } else if !self.theme_editor_ids.is_empty() {
                        self.active_theme_editor_id = self.theme_editor_ids.first().cloned();
                        ActiveWorkspaceTab::ThemeCss
                    } else if self.snippets_open {
                        ActiveWorkspaceTab::Snippets
                    } else if self.welcome_visible {
                        ActiveWorkspaceTab::Welcome
                    } else {
                        ActiveWorkspaceTab::Request
                    };
                }
            }
            WorkspaceToolTab::Settings => {
                if !self.settings_open {
                    return false;
                }
                self.settings_open = false;
                if self.active == ActiveWorkspaceTab::Settings {
                    self.active = if self.request_proxy_open {
                        ActiveWorkspaceTab::RequestProxy
                    } else if !self.theme_editor_ids.is_empty() {
                        self.active_theme_editor_id = self.theme_editor_ids.first().cloned();
                        ActiveWorkspaceTab::ThemeCss
                    } else if self.snippets_open {
                        ActiveWorkspaceTab::Snippets
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
                    } else if self.request_proxy_open {
                        self.active_theme_editor_id = None;
                        ActiveWorkspaceTab::RequestProxy
                    } else if self.snippets_open {
                        self.active_theme_editor_id = None;
                        ActiveWorkspaceTab::Snippets
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
        self.theme_editor_ids[index] = new_id.clone();
        if let Some(tab_order) = self.tab_order.as_mut() {
            let old_tab = WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss(old_id.to_owned()));
            let new_tab = WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss(new_id));
            if let Some(tab) = tab_order.iter_mut().find(|tab| **tab == old_tab) {
                *tab = new_tab;
            }
        }
        true
    }

    fn replace_ordered_tab(&mut self, old_tab: &WorkspaceTab, new_tab: WorkspaceTab) {
        let Some(tab_order) = self.tab_order.as_mut() else {
            return;
        };
        let Some(index) = tab_order.iter().position(|tab| tab == old_tab) else {
            return;
        };
        tab_order.remove(index);
        let mut insertion_index = index;
        if let Some(existing_index) = tab_order.iter().position(|tab| tab == &new_tab) {
            tab_order.remove(existing_index);
            if existing_index < insertion_index {
                insertion_index -= 1;
            }
        }
        tab_order.insert(insertion_index.min(tab_order.len()), new_tab);
    }

    pub fn active_tab(&self, request_tabs: &RequestTabs) -> WorkspaceTab {
        match self.active {
            ActiveWorkspaceTab::Welcome => WorkspaceTab::Welcome,
            ActiveWorkspaceTab::Request => {
                WorkspaceTab::Request(request_tabs.active_tab_id().clone())
            }
            ActiveWorkspaceTab::Snippets => WorkspaceTab::Tool(WorkspaceToolTab::Snippets),
            ActiveWorkspaceTab::RequestProxy => WorkspaceTab::Tool(WorkspaceToolTab::RequestProxy),
            ActiveWorkspaceTab::Settings => WorkspaceTab::Tool(WorkspaceToolTab::Settings),
            ActiveWorkspaceTab::ThemeCss => WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss(
                self.active_theme_editor_id
                    .as_ref()
                    .expect("active theme editor must have a tab identity")
                    .clone(),
            )),
        }
    }

    fn default_visible_tabs(&self, request_tabs: &RequestTabs) -> Vec<WorkspaceTab> {
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
        if self.snippets_open {
            tabs.push(WorkspaceTab::Tool(WorkspaceToolTab::Snippets));
        }
        if self.request_proxy_open {
            tabs.push(WorkspaceTab::Tool(WorkspaceToolTab::RequestProxy));
        }
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

    /// Return every open tab control in its shared visual order.
    ///
    /// Content-specific models decide whether a tab is currently open. The
    /// optional strip order only arranges those identities; stale identities
    /// are discarded and newly opened tabs are appended exactly once.
    pub fn visible_tabs(&self, request_tabs: &RequestTabs) -> Vec<WorkspaceTab> {
        let available = self.default_visible_tabs(request_tabs);
        let Some(tab_order) = self.tab_order.as_ref() else {
            return available;
        };

        let mut visible = Vec::with_capacity(available.len());
        for tab in tab_order {
            if available.contains(tab) && !visible.contains(tab) {
                visible.push(tab.clone());
            }
        }
        for tab in available {
            if !visible.contains(&tab) {
                visible.push(tab);
            }
        }
        visible
    }

    /// Move any workspace tab control before or after any other workspace tab.
    ///
    /// Request, Welcome, and runtime tool tabs all use this same operation.
    /// Their content models remain independent from strip presentation.
    pub fn reorder_tab(
        &mut self,
        request_tabs: &RequestTabs,
        dragged: &WorkspaceTab,
        target: &WorkspaceTab,
        after: bool,
    ) -> bool {
        if dragged == target {
            return false;
        }

        let previous = self.visible_tabs(request_tabs);
        let Some(source_index) = previous.iter().position(|tab| tab == dragged) else {
            return false;
        };
        if !previous.iter().any(|tab| tab == target) {
            return false;
        }

        let mut reordered = previous.clone();
        let dragged = reordered.remove(source_index);
        let target_index = reordered
            .iter()
            .position(|tab| tab == target)
            .expect("the distinct target must remain after removing the dragged tab");
        reordered.insert(target_index + usize::from(after), dragged);
        if reordered == previous {
            return false;
        }
        self.tab_order = Some(reordered);
        true
    }

    /// Mirror a moved request control into the durable request-tab sequence.
    ///
    /// The closest request to the right is the preferred stable anchor; when
    /// none exists, the closest request to the left is used. Tool and Welcome
    /// controls are deliberately ignored when choosing the durable anchor.
    pub fn align_request_tab_order(
        &self,
        request_tabs: &mut RequestTabs,
        dragged_id: &RequestTabId,
    ) -> bool {
        let visible = self.visible_tabs(request_tabs);
        let Some(dragged_index) = visible
            .iter()
            .position(|tab| tab == &WorkspaceTab::Request(dragged_id.clone()))
        else {
            return false;
        };
        let next_request = visible[dragged_index + 1..]
            .iter()
            .find_map(|tab| match tab {
                WorkspaceTab::Request(id) => Some(id.clone()),
                WorkspaceTab::Welcome | WorkspaceTab::Tool(_) => None,
            });
        if let Some(next_request) = next_request {
            return request_tabs.reorder_tab_before(dragged_id, &next_request);
        }
        let previous_request = visible[..dragged_index]
            .iter()
            .rev()
            .find_map(|tab| match tab {
                WorkspaceTab::Request(id) => Some(id.clone()),
                WorkspaceTab::Welcome | WorkspaceTab::Tool(_) => None,
            });
        previous_request.is_some_and(|previous_request| {
            request_tabs.reorder_tab_after(dragged_id, &previous_request)
        })
    }

    /// Resolve a workspace close scope in visible tab order without mutating state.
    /// Welcome is a presentation surface and is never a close target.
    pub fn close_targets(
        &self,
        request_tabs: &RequestTabs,
        anchor: &WorkspaceTab,
        scope: WorkspaceTabCloseScope,
    ) -> Vec<WorkspaceTab> {
        let visible_tabs = self.visible_tabs(request_tabs);
        let Some(anchor_index) = visible_tabs.iter().position(|tab| tab == anchor) else {
            return Vec::new();
        };

        visible_tabs
            .into_iter()
            .enumerate()
            .filter(|(index, tab)| {
                !matches!(tab, WorkspaceTab::Welcome)
                    && match scope {
                        WorkspaceTabCloseScope::Current => *index == anchor_index,
                        WorkspaceTabCloseScope::Others => *index != anchor_index,
                        WorkspaceTabCloseScope::ToLeft => *index < anchor_index,
                        WorkspaceTabCloseScope::ToRight => *index > anchor_index,
                        WorkspaceTabCloseScope::All => true,
                    }
            })
            .map(|(_, tab)| tab)
            .collect()
    }

    /// Choose the surviving visual neighbor when the active tab is closed.
    ///
    /// All tab kinds use the same right-then-left rule. `None` means either
    /// the active tab survives or no currently visible tab will survive.
    pub fn fallback_after_closing(
        &self,
        request_tabs: &RequestTabs,
        closing: &[WorkspaceTab],
    ) -> Option<WorkspaceTab> {
        let visible = self.visible_tabs(request_tabs);
        let active = self.active_tab(request_tabs);
        let active_index = visible.iter().position(|tab| tab == &active)?;
        if !closing.contains(&active) {
            return None;
        }

        visible[active_index + 1..]
            .iter()
            .find(|tab| !closing.contains(tab))
            .or_else(|| {
                visible[..active_index]
                    .iter()
                    .rev()
                    .find(|tab| !closing.contains(tab))
            })
            .cloned()
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

        tabs.open_tool(WorkspaceToolTab::RequestProxy);
        tabs.open_tool(WorkspaceToolTab::RequestProxy);
        assert_eq!(tabs.active(), ActiveWorkspaceTab::RequestProxy);
        assert_eq!(
            tabs.visible_tabs(&requests),
            vec![
                WorkspaceTab::Request(requests.active_tab_id().clone()),
                WorkspaceTab::Tool(WorkspaceToolTab::RequestProxy),
                WorkspaceTab::Tool(WorkspaceToolTab::Settings),
            ]
        );

        tabs.open_tool(WorkspaceToolTab::Snippets);
        tabs.open_tool(WorkspaceToolTab::Snippets);
        assert_eq!(tabs.active(), ActiveWorkspaceTab::Snippets);
        assert_eq!(
            tabs.visible_tabs(&requests),
            vec![
                WorkspaceTab::Request(requests.active_tab_id().clone()),
                WorkspaceTab::Tool(WorkspaceToolTab::Snippets),
                WorkspaceTab::Tool(WorkspaceToolTab::RequestProxy),
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
                WorkspaceTab::Tool(WorkspaceToolTab::Snippets),
                WorkspaceTab::Tool(WorkspaceToolTab::RequestProxy),
                WorkspaceTab::Tool(WorkspaceToolTab::Settings),
                WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss("ocean".to_owned())),
                WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss("forest".to_owned())),
            ]
        );
        assert!(tabs.tool_is_active(&WorkspaceToolTab::ThemeCss("forest".to_owned())));
        tabs.open_tool(WorkspaceToolTab::ThemeCss("ocean".to_owned()));
        assert_eq!(tabs.theme_editor_ids, ["ocean", "forest"]);
        assert!(tabs.tool_is_active(&WorkspaceToolTab::ThemeCss("ocean".to_owned())));
    }

    #[test]
    fn request_proxy_is_a_closeable_singleton_workspace_tool() {
        let requests = RequestTabs::default();
        let mut tabs = WorkspaceTabs::default();

        tabs.open_tool(WorkspaceToolTab::RequestProxy);
        tabs.open_tool(WorkspaceToolTab::RequestProxy);
        assert!(tabs.request_proxy_open());
        assert!(tabs.tool_is_active(&WorkspaceToolTab::RequestProxy));
        assert_eq!(
            tabs.visible_tabs(&requests),
            vec![
                WorkspaceTab::Request(requests.active_tab_id().clone()),
                WorkspaceTab::Tool(WorkspaceToolTab::RequestProxy),
            ]
        );

        assert!(tabs.close_tool(&WorkspaceToolTab::RequestProxy));
        assert_eq!(tabs.active(), ActiveWorkspaceTab::Request);
        assert!(!tabs.request_proxy_open());
        assert!(!tabs.close_tool(&WorkspaceToolTab::RequestProxy));
    }

    #[test]
    fn every_tab_kind_reorders_in_one_visual_sequence_without_changing_active_tab() {
        let mut requests = RequestTabs::default();
        let first = requests.active_tab_id().clone();
        let second = requests.open_new();
        let mut tabs = WorkspaceTabs::default();
        tabs.open_tool(WorkspaceToolTab::Snippets);
        tabs.open_tool(WorkspaceToolTab::Settings);
        tabs.open_tool(WorkspaceToolTab::ThemeCss("ocean".to_owned()));
        tabs.open_tool(WorkspaceToolTab::Settings);

        let snippets = WorkspaceTab::Tool(WorkspaceToolTab::Snippets);
        let settings = WorkspaceTab::Tool(WorkspaceToolTab::Settings);
        let theme = WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss("ocean".to_owned()));
        let first = WorkspaceTab::Request(first);
        let second = WorkspaceTab::Request(second);
        let active_before = tabs.active_tab(&requests);

        assert!(tabs.reorder_tab(&requests, &snippets, &first, false));
        assert!(tabs.reorder_tab(&requests, &second, &theme, true));
        assert!(tabs.reorder_tab(&requests, &theme, &settings, false));
        assert!(tabs.reorder_tab(&requests, &second, &first, false));
        assert_eq!(
            tabs.visible_tabs(&requests),
            vec![
                snippets.clone(),
                second.clone(),
                first.clone(),
                theme.clone(),
                settings.clone(),
            ]
        );
        assert_eq!(tabs.active_tab(&requests), active_before);

        assert!(!tabs.reorder_tab(&requests, &settings, &settings, false));
        assert!(!tabs.reorder_tab(
            &requests,
            &WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss("missing".to_owned())),
            &settings,
            false,
        ));
        assert!(!tabs.reorder_tab(
            &requests,
            &settings,
            &WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss("missing".to_owned())),
            false,
        ));

        assert_eq!(tabs.adjacent_tab(&requests, 1), snippets);
        assert_eq!(tabs.adjacent_tab(&requests, -1), theme);
        assert_eq!(
            tabs.close_targets(&requests, &first, WorkspaceTabCloseScope::ToLeft),
            vec![WorkspaceTab::Tool(WorkspaceToolTab::Snippets), second,]
        );
        assert_eq!(
            tabs.fallback_after_closing(&requests, std::slice::from_ref(&settings)),
            Some(WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss(
                "ocean".to_owned()
            )))
        );
    }

    #[test]
    fn welcome_uses_the_same_order_and_keeps_its_slot_when_revealing_backing_request() {
        let requests = requests_with_welcome();
        let backing_id = requests.active_tab_id().clone();
        let mut tabs = WorkspaceTabs::from_request_tabs(&requests);
        tabs.open_tool(WorkspaceToolTab::Snippets);
        tabs.open_tool(WorkspaceToolTab::Settings);
        let settings = WorkspaceTab::Tool(WorkspaceToolTab::Settings);

        assert!(tabs.reorder_tab(&requests, &WorkspaceTab::Welcome, &settings, true));
        assert_eq!(
            tabs.visible_tabs(&requests),
            vec![
                WorkspaceTab::Tool(WorkspaceToolTab::Snippets),
                settings.clone(),
                WorkspaceTab::Welcome,
            ]
        );

        assert!(tabs.browse_requests());
        assert_eq!(
            tabs.visible_tabs(&requests),
            vec![
                WorkspaceTab::Tool(WorkspaceToolTab::Snippets),
                settings,
                WorkspaceTab::Request(backing_id.clone()),
            ]
        );
        tabs.open_welcome(backing_id, false);
        assert_eq!(
            tabs.visible_tabs(&requests).last(),
            Some(&WorkspaceTab::Welcome)
        );
    }

    #[test]
    fn request_projection_follows_a_request_dragged_across_tool_tabs() {
        let mut requests = RequestTabs::default();
        let first = requests.active_tab_id().clone();
        let second = requests.open_new();
        let third = requests.open_new();
        let active_before = requests.active_tab_id().clone();
        let mut tabs = WorkspaceTabs::default();
        tabs.open_tool(WorkspaceToolTab::Settings);

        assert!(tabs.reorder_tab(
            &requests,
            &WorkspaceTab::Request(first.clone()),
            &WorkspaceTab::Tool(WorkspaceToolTab::Settings),
            true,
        ));
        assert!(tabs.align_request_tab_order(&mut requests, &first));
        assert_eq!(
            requests
                .tabs()
                .iter()
                .map(|tab| tab.id().clone())
                .collect::<Vec<_>>(),
            vec![second, third, first]
        );
        assert_eq!(requests.active_tab_id(), &active_before);

        let restored: RequestTabs =
            serde_json::from_str(&serde_json::to_string(&requests).unwrap()).unwrap();
        assert_eq!(restored, requests);
    }

    #[test]
    fn reopening_tools_and_replacing_theme_identity_preserve_shared_positions() {
        let requests = RequestTabs::default();
        let request = WorkspaceTab::Request(requests.active_tab_id().clone());
        let mut tabs = WorkspaceTabs::default();
        tabs.open_tool(WorkspaceToolTab::Settings);
        tabs.open_tool(WorkspaceToolTab::ThemeCss("ocean".to_owned()));
        let settings = WorkspaceTab::Tool(WorkspaceToolTab::Settings);
        let ocean = WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss("ocean".to_owned()));

        assert!(tabs.reorder_tab(&requests, &ocean, &request, false));
        let before_reopen = tabs.visible_tabs(&requests);
        tabs.open_tool(WorkspaceToolTab::ThemeCss("ocean".to_owned()));
        assert_eq!(tabs.visible_tabs(&requests), before_reopen);

        assert!(tabs.replace_theme_editor_id("ocean", "forest".to_owned()));
        assert_eq!(
            tabs.visible_tabs(&requests),
            vec![
                WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss("forest".to_owned())),
                request,
                settings,
            ]
        );
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
    fn singleton_snippets_follow_visual_neighbor_close_fallbacks() {
        let requests = RequestTabs::default();
        let mut tabs = WorkspaceTabs::default();
        tabs.open_tool(WorkspaceToolTab::ThemeCss("ocean".to_owned()));
        tabs.open_tool(WorkspaceToolTab::Settings);
        tabs.open_tool(WorkspaceToolTab::Snippets);

        assert_eq!(
            tabs.visible_tabs(&requests),
            vec![
                WorkspaceTab::Request(requests.active_tab_id().clone()),
                WorkspaceTab::Tool(WorkspaceToolTab::Snippets),
                WorkspaceTab::Tool(WorkspaceToolTab::Settings),
                WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss("ocean".to_owned())),
            ]
        );
        assert!(tabs.tool_is_active(&WorkspaceToolTab::Snippets));

        assert!(tabs.close_tool(&WorkspaceToolTab::Snippets));
        assert_eq!(tabs.active(), ActiveWorkspaceTab::Settings);
        assert!(!tabs.snippets_open());
        assert!(!tabs.close_tool(&WorkspaceToolTab::Snippets));

        tabs.open_tool(WorkspaceToolTab::Snippets);
        assert!(tabs.close_tool(&WorkspaceToolTab::Settings));
        assert_eq!(tabs.active(), ActiveWorkspaceTab::Snippets);
        assert!(tabs.close_tool(&WorkspaceToolTab::Snippets));
        assert!(tabs.tool_is_active(&WorkspaceToolTab::ThemeCss("ocean".to_owned())));
    }

    #[test]
    fn closing_settings_falls_back_to_snippets_without_a_right_hand_tool() {
        let mut tabs = WorkspaceTabs::default();
        tabs.open_tool(WorkspaceToolTab::Snippets);
        tabs.open_tool(WorkspaceToolTab::Settings);

        assert!(tabs.close_tool(&WorkspaceToolTab::Settings));
        assert_eq!(tabs.active(), ActiveWorkspaceTab::Snippets);
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
    fn mixed_close_targets_follow_visual_order_from_a_request_anchor() {
        let mut requests = RequestTabs::default();
        let first = requests.active_tab_id().clone();
        let second = requests.open_new();
        let third = requests.open_new();
        let mut tabs = WorkspaceTabs::default();
        tabs.open_tool(WorkspaceToolTab::Settings);
        tabs.open_tool(WorkspaceToolTab::ThemeCss("ocean".to_owned()));
        tabs.open_tool(WorkspaceToolTab::ThemeCss("forest".to_owned()));
        let anchor = WorkspaceTab::Request(second.clone());
        let settings = WorkspaceTab::Tool(WorkspaceToolTab::Settings);
        let ocean = WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss("ocean".to_owned()));
        let forest = WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss("forest".to_owned()));

        assert_eq!(
            tabs.close_targets(&requests, &anchor, WorkspaceTabCloseScope::Current),
            vec![anchor.clone()]
        );
        assert_eq!(
            tabs.close_targets(&requests, &anchor, WorkspaceTabCloseScope::Others),
            vec![
                WorkspaceTab::Request(first.clone()),
                WorkspaceTab::Request(third.clone()),
                settings.clone(),
                ocean.clone(),
                forest.clone(),
            ]
        );
        assert_eq!(
            tabs.close_targets(&requests, &anchor, WorkspaceTabCloseScope::ToLeft),
            vec![WorkspaceTab::Request(first.clone())]
        );
        assert_eq!(
            tabs.close_targets(&requests, &anchor, WorkspaceTabCloseScope::ToRight),
            vec![
                WorkspaceTab::Request(third.clone()),
                settings.clone(),
                ocean.clone(),
                forest.clone(),
            ]
        );
        assert_eq!(
            tabs.close_targets(&requests, &anchor, WorkspaceTabCloseScope::All),
            vec![
                WorkspaceTab::Request(first),
                anchor,
                WorkspaceTab::Request(third),
                settings,
                ocean,
                forest,
            ]
        );
    }

    #[test]
    fn mixed_close_targets_follow_visual_order_from_a_tool_anchor() {
        let mut requests = RequestTabs::default();
        let first = requests.active_tab_id().clone();
        let second = requests.open_new();
        let third = requests.open_new();
        let mut tabs = WorkspaceTabs::default();
        tabs.open_tool(WorkspaceToolTab::Settings);
        tabs.open_tool(WorkspaceToolTab::ThemeCss("ocean".to_owned()));
        tabs.open_tool(WorkspaceToolTab::ThemeCss("forest".to_owned()));
        let settings = WorkspaceTab::Tool(WorkspaceToolTab::Settings);
        let anchor = WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss("ocean".to_owned()));
        let forest = WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss("forest".to_owned()));

        assert_eq!(
            tabs.close_targets(&requests, &anchor, WorkspaceTabCloseScope::Current),
            vec![anchor.clone()]
        );
        assert_eq!(
            tabs.close_targets(&requests, &anchor, WorkspaceTabCloseScope::Others),
            vec![
                WorkspaceTab::Request(first.clone()),
                WorkspaceTab::Request(second.clone()),
                WorkspaceTab::Request(third.clone()),
                settings.clone(),
                forest.clone(),
            ]
        );
        assert_eq!(
            tabs.close_targets(&requests, &anchor, WorkspaceTabCloseScope::ToLeft),
            vec![
                WorkspaceTab::Request(first.clone()),
                WorkspaceTab::Request(second.clone()),
                WorkspaceTab::Request(third.clone()),
                settings.clone(),
            ]
        );
        assert_eq!(
            tabs.close_targets(&requests, &anchor, WorkspaceTabCloseScope::ToRight),
            vec![forest.clone()]
        );
        assert_eq!(
            tabs.close_targets(&requests, &anchor, WorkspaceTabCloseScope::All),
            vec![
                WorkspaceTab::Request(first),
                WorkspaceTab::Request(second),
                WorkspaceTab::Request(third),
                settings,
                anchor,
                forest,
            ]
        );
    }

    #[test]
    fn close_targets_never_include_welcome() {
        let mut requests = RequestTabs::default();
        let welcome_backing = requests.active_tab_id().clone();
        let request = requests.open_new();
        let mut tabs = WorkspaceTabs::default();
        tabs.open_welcome(welcome_backing, false);
        tabs.open_tool(WorkspaceToolTab::Settings);
        tabs.open_tool(WorkspaceToolTab::ThemeCss("ocean".to_owned()));
        let anchor = WorkspaceTab::Tool(WorkspaceToolTab::Settings);

        assert_eq!(
            tabs.close_targets(&requests, &anchor, WorkspaceTabCloseScope::All),
            vec![
                WorkspaceTab::Request(request),
                anchor,
                WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss("ocean".to_owned())),
            ]
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
