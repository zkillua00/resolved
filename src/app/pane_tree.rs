use std::sync::atomic::{AtomicU64, Ordering};

use super::workspace_tab::WorkspaceTab;

/// Identity for one pane in the split layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct PaneId(pub(super) u64);

static NEXT_PANE_ID: AtomicU64 = AtomicU64::new(0);

impl PaneId {
    fn new() -> Self {
        Self(NEXT_PANE_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// Orientation of a split container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SplitDirection {
    Horizontal,
    Vertical,
}

/// A workspace tab identity held by a pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PaneTab {
    tab: WorkspaceTab,
}

/// A leaf pane that owns an ordered list of tabs plus its own active tab.
#[derive(Clone, Debug)]
pub(super) struct Pane {
    id: PaneId,
    tabs: Vec<PaneTab>,
    active_index: usize,
}

/// A split container holding two or more child pane roots.
#[derive(Clone, Debug)]
pub(super) struct Split {
    pub(super) direction: SplitDirection,
    pub(super) children: Vec<PaneRoot>,
}

/// The hierarchical pane layout used for the workspace.
///
/// [`PaneRoot::Leaf`] is the common single-pane case; [`PaneRoot::Split`]
/// holds the children of a horizontal or vertical split. Tabs, including the
/// Open snippets/settings/theme tool singletons, live in exactly one pane so
/// that request ids and tool singletons are never duplicated across panes.
#[derive(Clone, Debug)]
pub(super) enum PaneRoot {
    Leaf(Pane),
    Split(Box<Split>),
}

/// Errors reported by pane-tree mutations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PaneError {
    PaneNotFound,
}

impl Pane {
    pub(super) fn new() -> Self {
        Self {
            id: PaneId::new(),
            tabs: Vec::new(),
            active_index: 0,
        }
    }

    pub(super) fn id(&self) -> PaneId {
        self.id
    }

    pub(super) fn tabs(&self) -> Vec<WorkspaceTab> {
        self.tabs.iter().map(|entry| entry.tab.clone()).collect()
    }

    pub(super) fn contains(&self, tab: &WorkspaceTab) -> bool {
        self.tabs.iter().any(|entry| &entry.tab == tab)
    }

    pub(super) fn active_tab(&self) -> Option<WorkspaceTab> {
        self.tabs
            .get(self.active_index)
            .map(|entry| entry.tab.clone())
    }

    fn push_tab(&mut self, tab: WorkspaceTab) {
        if self.tabs.is_empty() {
            self.active_index = 0;
        }
        self.tabs.push(PaneTab { tab });
    }

    /// Insert `tab`, activating it; activating an existing tab selects it.
    pub(super) fn insert_or_activate(&mut self, tab: WorkspaceTab) {
        if let Some(position) = self.tabs.iter().position(|entry| entry.tab == tab) {
            self.active_index = position;
            return;
        }
        self.push_tab(tab);
        self.active_index = self.tabs.len() - 1;
    }

    fn clamp_active(&mut self) {
        if self.tabs.is_empty() {
            self.active_index = 0;
        } else if self.active_index >= self.tabs.len() {
            self.active_index = self.tabs.len() - 1;
        }
    }
}

impl PaneRoot {
    /// A fresh root containing a single empty pane.
    pub(super) fn new_leaf() -> Self {
        Self::Leaf(Pane::new())
    }

    /// Build a single-pane root from a list of tabs, activating the given index.
    pub(super) fn from_tabs(tabs: Vec<WorkspaceTab>, active_index: usize) -> Self {
        let mut pane = Pane::new();
        for tab in tabs {
            pane.push_tab(tab);
        }
        pane.active_index = active_index.min(pane.tabs.len().saturating_sub(1));
        Self::Leaf(pane)
    }

    /// All panes in depth-first order.
    pub(super) fn panes(&self) -> Vec<&Pane> {
        let mut panes = Vec::new();
        self.collect_panes(&mut panes);
        panes
    }

    /// All panes in depth-first order, mutably.
    pub(super) fn panes_mut(&mut self) -> Vec<&mut Pane> {
        let mut panes = Vec::new();
        self.collect_panes_mut(&mut panes);
        panes
    }

    pub(super) fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes().into_iter().find(|pane| pane.id == id)
    }

    pub(super) fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.panes_mut().into_iter().find(|pane| pane.id == id)
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Leaf(_) => 1,
            Self::Split(_) => self.panes().len(),
        }
    }

    pub(super) fn is_single_leaf(&self) -> bool {
        matches!(self, Self::Leaf(_))
    }

    /// The pane that currently owns `tab`, if any.
    pub(super) fn pane_for_tab(&self, tab: &WorkspaceTab) -> Option<PaneId> {
        self.panes()
            .into_iter()
            .find(|pane| pane.contains(tab))
            .map(|pane| pane.id)
    }

    /// Flatten every open tab across all panes.
    pub(super) fn active_tabs(&self) -> Vec<WorkspaceTab> {
        self.panes()
            .into_iter()
            .flat_map(|pane| pane.tabs())
            .collect()
    }

    /// Reconcile pane membership with the tabs that are currently open.
    ///
    /// The workspace tab model remains the authority for open/closed tabs,
    /// while this tree owns their pane placement. This keeps those two models
    /// aligned after ordinary tab actions (open, close, Welcome replacement)
    /// that do not otherwise mutate the split tree.
    pub(super) fn reconcile_open_tabs(
        &mut self,
        open_tabs: &[WorkspaceTab],
        active_tab: &WorkspaceTab,
    ) -> bool {
        if let Self::Leaf(pane) = self {
            let desired = open_tabs
                .iter()
                .cloned()
                .map(|tab| PaneTab { tab })
                .collect::<Vec<_>>();
            let active_index = open_tabs
                .iter()
                .position(|tab| tab == active_tab)
                .unwrap_or(0);
            if pane.tabs == desired && pane.active_index == active_index {
                return false;
            }
            pane.tabs = desired;
            pane.active_index = active_index;
            pane.clamp_active();
            return true;
        }

        let open = open_tabs.iter().collect::<std::collections::HashSet<_>>();
        let mut changed = false;
        let mut removed_tabs = false;
        let mut vacated_panes = Vec::new();
        for pane in self.panes_mut() {
            let previous_active = pane.active_tab();
            let previous_len = pane.tabs.len();
            pane.tabs.retain(|entry| open.contains(&entry.tab));
            if pane.tabs.len() != previous_len {
                changed = true;
                removed_tabs = true;
                if pane.tabs.is_empty() {
                    vacated_panes.push(pane.id);
                }
            }
            pane.active_index = previous_active
                .and_then(|tab| pane.tabs.iter().position(|entry| entry.tab == tab))
                .unwrap_or(0);
            pane.clamp_active();
        }

        let existing = self
            .active_tabs()
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        let missing = open_tabs
            .iter()
            .filter(|tab| !existing.contains(*tab))
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            let target_id = vacated_panes
                .first()
                .copied()
                .or_else(|| self.pane_for_tab(active_tab))
                .or_else(|| self.panes().first().map(|pane| pane.id));
            if let Some(target_id) = target_id
                && let Some(pane) = self.pane_mut(target_id)
            {
                for tab in missing {
                    pane.push_tab(tab);
                }
                changed = true;
            }
        }

        if let Some(pane_id) = self.pane_for_tab(active_tab)
            && let Some(pane) = self.pane_mut(pane_id)
            && let Some(index) = pane.tabs.iter().position(|entry| &entry.tab == active_tab)
            && pane.active_index != index
        {
            pane.active_index = index;
            changed = true;
        }
        if removed_tabs {
            self.remove_empty_panes();
        }
        changed
    }

    /// Ensure no request id and no tool singleton is duplicated across panes.
    #[cfg(test)]
    pub(super) fn validate_tab_uniqueness(&self) -> bool {
        let mut seen = std::collections::HashSet::new();
        for tab in self.active_tabs() {
            if !seen.insert(tab) {
                return false;
            }
        }
        true
    }

    /// Reorder `dragged` before/after `target` within the pane `pane_id`.
    pub(super) fn reorder_within_pane(
        &mut self,
        pane_id: PaneId,
        dragged: &WorkspaceTab,
        target: &WorkspaceTab,
        after: bool,
    ) -> bool {
        if dragged == target {
            return false;
        }
        for pane in self.panes_mut() {
            if pane.id != pane_id {
                continue;
            }
            let Some(source_index) = pane.tabs.iter().position(|entry| &entry.tab == dragged)
            else {
                return false;
            };
            if !pane.tabs.iter().any(|entry| &entry.tab == target) {
                return false;
            }
            let active_tab = pane
                .tabs
                .get(pane.active_index.min(pane.tabs.len().saturating_sub(1)))
                .map(|entry| entry.tab.clone());
            let mut tabs = pane.tabs.clone();
            let dragged_tab = tabs.remove(source_index);
            let target_index = tabs
                .iter()
                .position(|entry| &entry.tab == target)
                .expect("the distinct target must remain after removing the dragged tab");
            tabs.insert(target_index + usize::from(after), dragged_tab);
            if tabs == pane.tabs {
                return false;
            }
            pane.tabs = tabs;
            pane.active_index = active_tab
                .and_then(|tab| pane.tabs.iter().position(|entry| entry.tab == tab))
                .unwrap_or(0);
            pane.clamp_active();
            return true;
        }
        false
    }

    /// Move `dragged` from `from_pane_id` into `to_pane_id` at `index`.
    ///
    /// Removing the tab may leave the source pane empty; in that case the
    /// empty pane is removed and the tree is collapsed.
    pub(super) fn move_tab_between_panes(
        &mut self,
        dragged: &WorkspaceTab,
        from_pane_id: PaneId,
        to_pane_id: PaneId,
        index: usize,
    ) -> bool {
        if from_pane_id == to_pane_id {
            return false;
        }
        let mut removed = false;
        for pane in self.panes_mut() {
            if pane.id == from_pane_id {
                let Some(position) = pane.tabs.iter().position(|entry| &entry.tab == dragged)
                else {
                    return false;
                };
                pane.tabs.remove(position);
                if pane.active_index >= position && pane.active_index > 0 {
                    pane.active_index -= 1;
                }
                pane.clamp_active();
                removed = true;
                break;
            }
        }
        if !removed {
            return false;
        }

        let mut inserted = false;
        for pane in self.panes_mut() {
            if pane.id == to_pane_id {
                if pane.tabs.iter().any(|entry| &entry.tab == dragged) {
                    return false;
                }
                let index = index.min(pane.tabs.len());
                pane.tabs.insert(
                    index,
                    PaneTab {
                        tab: dragged.clone(),
                    },
                );
                if pane.tabs.len() == 1 {
                    pane.active_index = 0;
                } else if index <= pane.active_index {
                    pane.active_index += 1;
                }
                inserted = true;
                break;
            }
        }
        if !inserted {
            return false;
        }

        self.remove_empty_panes();
        true
    }

    /// Remove `tab` from the pane `pane_id` without collapsing the tree.
    ///
    /// Used when moving the single tab of a freshly split pane: the creator
    /// intentionally asked for two panes, so an empty source pane is kept
    /// instead of being collapsed away.
    pub(super) fn remove_tab_from_pane(&mut self, tab: &WorkspaceTab, pane_id: PaneId) -> bool {
        for pane in self.panes_mut() {
            if pane.id != pane_id {
                continue;
            }
            let Some(position) = pane.tabs.iter().position(|entry| &entry.tab == tab) else {
                return false;
            };
            pane.tabs.remove(position);
            if pane.active_index >= position && pane.active_index > 0 {
                pane.active_index -= 1;
            }
            pane.clamp_active();
            return true;
        }
        false
    }

    /// Replace the anchor leaf with a split container holding the anchor pane
    /// and a fresh pane; return the new pane's id so the dragged tab can be
    /// inserted into it next.
    pub(super) fn split_off_pane(
        &mut self,
        anchor_pane_id: PaneId,
        direction: SplitDirection,
        after: bool,
    ) -> Result<PaneId, PaneError> {
        let Some(_) = self.pane(anchor_pane_id) else {
            return Err(PaneError::PaneNotFound);
        };
        let current = std::mem::replace(self, Self::new_leaf());
        let (rebuilt, new_id) = split_into(current, anchor_pane_id, direction, after);
        *self = rebuilt;
        new_id.ok_or(PaneError::PaneNotFound)
    }

    /// Close a tab wherever it lives, removing the pane if it becomes empty.
    #[cfg(test)]
    pub(super) fn close_tab(&mut self, tab: &WorkspaceTab) -> bool {
        let mut found = false;
        for pane in self.panes_mut() {
            let Some(position) = pane.tabs.iter().position(|entry| &entry.tab == tab) else {
                continue;
            };
            pane.tabs.remove(position);
            if position < pane.active_index && pane.active_index > 0 {
                pane.active_index -= 1;
            }
            pane.clamp_active();
            found = true;
        }
        if found {
            self.remove_empty_panes();
        }
        found
    }

    /// Remove empty panes and collapse any split with fewer than two children.
    pub(super) fn remove_empty_panes(&mut self) {
        let current = std::mem::replace(self, Self::new_leaf());
        *self = normalize(current);
    }

    fn collect_panes<'a>(&'a self, out: &mut Vec<&'a Pane>) {
        match self {
            Self::Leaf(pane) => out.push(pane),
            Self::Split(split) => {
                for child in &split.children {
                    child.collect_panes(out);
                }
            }
        }
    }

    fn collect_panes_mut<'a>(&'a mut self, out: &mut Vec<&'a mut Pane>) {
        match self {
            Self::Leaf(pane) => out.push(pane),
            Self::Split(split) => {
                for child in &mut split.children {
                    child.collect_panes_mut(out);
                }
            }
        }
    }
}

fn split_into(
    root: PaneRoot,
    anchor: PaneId,
    direction: SplitDirection,
    after: bool,
) -> (PaneRoot, Option<PaneId>) {
    match root {
        PaneRoot::Leaf(pane) if pane.id == anchor => {
            let new_pane = Pane::new();
            let new_id = new_pane.id;
            let mut children = Vec::with_capacity(2);
            if after {
                children.push(PaneRoot::Leaf(pane));
                children.push(PaneRoot::Leaf(new_pane));
            } else {
                children.push(PaneRoot::Leaf(new_pane));
                children.push(PaneRoot::Leaf(pane));
            }
            (
                PaneRoot::Split(Box::new(Split {
                    direction,
                    children,
                })),
                Some(new_id),
            )
        }
        PaneRoot::Leaf(pane) => (PaneRoot::Leaf(pane), None),
        PaneRoot::Split(mut split) => {
            let mut found = None;
            for child in &mut split.children {
                if found.is_some() {
                    continue;
                }
                let replaced = std::mem::replace(child, PaneRoot::new_leaf());
                let (rebuilt, id) = split_into(replaced, anchor, direction, after);
                *child = rebuilt;
                found = id;
            }
            (PaneRoot::Split(split), found)
        }
    }
}

fn normalize(root: PaneRoot) -> PaneRoot {
    match root {
        PaneRoot::Leaf(pane) => PaneRoot::Leaf(pane),
        PaneRoot::Split(split) => {
            let direction = split.direction;
            let mut children = Vec::new();
            for child in split.children {
                match normalize(child) {
                    PaneRoot::Leaf(pane) if pane.tabs.is_empty() => {}
                    other => children.push(other),
                }
            }
            match children.len() {
                0 => PaneRoot::new_leaf(),
                1 => children.pop().expect("one child"),
                _ => PaneRoot::Split(Box::new(Split {
                    direction,
                    children,
                })),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::workspace_tab::WorkspaceToolTab;
    use crate::core::RequestTabId;

    fn tab(request: &RequestTabId) -> WorkspaceTab {
        WorkspaceTab::Request(request.clone())
    }

    fn single(request: &RequestTabId) -> PaneRoot {
        PaneRoot::from_tabs(vec![tab(request)], 0)
    }

    #[test]
    fn root_starts_as_a_single_pane() {
        let a = RequestTabId::new();
        let root = single(&a);
        assert!(root.is_single_leaf());
        assert_eq!(root.len(), 1);
        assert_eq!(root.active_tabs(), vec![tab(&a)]);
        assert!(root.validate_tab_uniqueness());
    }

    #[test]
    fn reorder_within_pane_moves_tabs_and_keeps_active_tab() {
        let a = RequestTabId::new();
        let b = RequestTabId::new();
        let c = RequestTabId::new();
        let mut root = PaneRoot::from_tabs(vec![tab(&a), tab(&b), tab(&c)], 0);
        let pane_id = root.panes()[0].id();
        assert!(root.reorder_within_pane(pane_id, &tab(&c), &tab(&a), false));
        assert_eq!(root.active_tabs(), vec![tab(&c), tab(&a), tab(&b)]);
        assert_eq!(
            root.pane(pane_id).and_then(|pane| pane.active_tab()),
            Some(tab(&a))
        );
        assert!(!root.reorder_within_pane(pane_id, &tab(&a), &tab(&a), false));
        let missing = RequestTabId::new();
        assert!(!root.reorder_within_pane(pane_id, &tab(&missing), &tab(&a), false));
    }

    #[test]
    fn split_off_creates_a_two_child_split_with_a_fresh_pane() {
        let a = RequestTabId::new();
        let mut root = single(&a);
        let anchor = root.panes()[0].id();
        let new_id = root
            .split_off_pane(anchor, SplitDirection::Vertical, true)
            .expect("the anchor pane must split");
        assert!(!root.is_single_leaf());
        assert_eq!(root.len(), 2);
        assert_ne!(new_id, anchor);
        assert!(root.pane(new_id).is_some());
        assert_eq!(
            root.pane(anchor).and_then(|pane| pane.active_tab()),
            Some(tab(&a))
        );
        assert_eq!(root.pane(new_id).and_then(|pane| pane.active_tab()), None);
        assert!(root.validate_tab_uniqueness());

        let _ = root.split_off_pane(PaneId::new(), SplitDirection::Horizontal, false);
    }

    #[test]
    fn move_tab_between_panes_removes_from_source_and_inserts_into_target() {
        let a = RequestTabId::new();
        let b = RequestTabId::new();
        let mut root = PaneRoot::from_tabs(vec![tab(&a), tab(&b)], 0);
        let first = root.panes()[0].id();
        let second = root
            .split_off_pane(first, SplitDirection::Vertical, true)
            .expect("split");
        let tab_b = tab(&b);
        assert!(root.move_tab_between_panes(&tab_b, first, second, 0));
        assert_eq!(
            root.pane(first).and_then(|pane| pane.active_tab()),
            Some(tab(&a))
        );
        assert_eq!(
            root.pane(second).and_then(|pane| pane.active_tab()),
            Some(tab(&b))
        );
        assert_eq!(root.active_tabs(), vec![tab(&a), tab(&b)]);
        assert!(root.validate_tab_uniqueness());
        assert!(!root.move_tab_between_panes(&tab_b, first, second, 0));
    }

    #[test]
    fn closing_the_last_tab_in_a_pane_collapses_the_split() {
        let a = RequestTabId::new();
        let b = RequestTabId::new();
        let mut root = PaneRoot::from_tabs(vec![tab(&a), tab(&b)], 0);
        let first = root.panes()[0].id();
        let second = root
            .split_off_pane(first, SplitDirection::Vertical, true)
            .expect("split");
        let tab_b = tab(&b);
        assert!(root.move_tab_between_panes(&tab_b, first, second, 0));
        assert_eq!(root.len(), 2);

        assert!(root.close_tab(&tab(&a)));
        assert!(root.is_single_leaf());
        assert_eq!(root.active_tabs(), vec![tab(&b)]);
        assert!(root.validate_tab_uniqueness());
    }

    #[test]
    fn tool_singletons_and_request_ids_stay_unique_across_panes() {
        let a = RequestTabId::new();
        let b = RequestTabId::new();
        let settings = WorkspaceTab::Tool(WorkspaceToolTab::Settings);
        let mut root = PaneRoot::Split(Box::new(Split {
            direction: SplitDirection::Vertical,
            children: vec![
                PaneRoot::from_tabs(vec![tab(&a), settings.clone()], 0),
                PaneRoot::from_tabs(vec![tab(&b)], 0),
            ],
        }));
        assert!(root.validate_tab_uniqueness());
        let first = root.panes()[0].id();
        let second = root.panes()[1].id();

        assert!(root.move_tab_between_panes(&tab(&a), first, second, 0));
        assert!(root.validate_tab_uniqueness());
        assert!(!root.move_tab_between_panes(&tab(&a), first, second, 0));

        assert!(root.move_tab_between_panes(&settings, first, second, 0));
        assert!(root.validate_tab_uniqueness());
    }

    #[test]
    fn validation_catches_a_duplicated_singleton_across_panes() {
        let a = RequestTabId::new();
        let settings = WorkspaceTab::Tool(WorkspaceToolTab::Settings);
        let root = PaneRoot::Split(Box::new(Split {
            direction: SplitDirection::Vertical,
            children: vec![
                PaneRoot::from_tabs(vec![tab(&a), settings.clone()], 0),
                PaneRoot::from_tabs(vec![settings.clone()], 0),
            ],
        }));
        assert!(!root.validate_tab_uniqueness());
    }

    #[test]
    fn close_is_idempotent_and_preserves_welcome() {
        let a = RequestTabId::new();
        let mut root = PaneRoot::from_tabs(vec![tab(&a), WorkspaceTab::Welcome], 0);
        assert!(root.close_tab(&tab(&a)));
        assert_eq!(root.active_tabs(), vec![WorkspaceTab::Welcome]);
        assert!(!root.close_tab(&tab(&a)));
    }

    #[test]
    fn splitting_out_the_only_tab_keeps_an_empty_source_pane() {
        let a = RequestTabId::new();
        let mut root = single(&a);
        let anchor = root.panes()[0].id();
        let new_id = root
            .split_off_pane(anchor, SplitDirection::Vertical, true)
            .expect("split");
        assert!(root.remove_tab_from_pane(&tab(&a), anchor));
        assert!(root.pane_mut(new_id).is_some());
        // An empty source pane survives: the user explicitly split, so the tree
        // must not collapse back to a single pane.
        assert!(!root.is_single_leaf());
        assert_eq!(root.len(), 2);
        assert_eq!(
            root.pane(anchor)
                .and_then(|pane| pane.tabs().first().cloned()),
            None
        );
        assert!(!root.remove_tab_from_pane(&tab(&a), anchor));
    }

    #[test]
    fn moving_a_split_tab_back_to_its_original_pane_collapses_the_split() {
        let a = RequestTabId::new();
        let b = RequestTabId::new();
        let mut root = PaneRoot::from_tabs(vec![tab(&a), tab(&b)], 0);
        let original = root.panes()[0].id();
        let split = root
            .split_off_pane(original, SplitDirection::Vertical, true)
            .expect("split");
        assert!(root.move_tab_between_panes(&tab(&b), original, split, 0));
        assert_eq!(root.len(), 2);

        assert!(root.move_tab_between_panes(&tab(&b), split, original, 1));
        assert!(root.is_single_leaf());
        assert_eq!(root.active_tabs(), vec![tab(&a), tab(&b)]);
    }

    #[test]
    fn reconciling_closed_tabs_collapses_empty_panes_and_keeps_new_tabs_reachable() {
        let a = RequestTabId::new();
        let b = RequestTabId::new();
        let replacement = RequestTabId::new();
        let mut root = PaneRoot::from_tabs(vec![tab(&a), tab(&b)], 0);
        let original = root.panes()[0].id();
        let split = root
            .split_off_pane(original, SplitDirection::Vertical, true)
            .expect("split");
        assert!(root.move_tab_between_panes(&tab(&b), original, split, 0));

        assert!(root.reconcile_open_tabs(&[tab(&a)], &tab(&a)));
        assert!(root.is_single_leaf());
        assert_eq!(root.active_tabs(), vec![tab(&a)]);

        assert!(root.reconcile_open_tabs(&[tab(&a), tab(&replacement)], &tab(&replacement),));
        assert_eq!(root.active_tabs(), vec![tab(&a), tab(&replacement)]);
        assert_eq!(
            root.panes()[0].active_tab(),
            Some(tab(&replacement)),
            "a newly opened active tab must be assigned to a live pane",
        );
    }
}
