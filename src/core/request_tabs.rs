use std::{
    collections::{HashMap, HashSet},
    sync::atomic::{AtomicU64, Ordering},
};

use chrono::Utc;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::template::RequestTemplate;

pub const DEFAULT_REQUEST_TAB_TITLE: &str = "Untitled Request";
pub const DEFAULT_REQUEST_TAB_GROUP_TITLE: &str = "Tab Group";

static NEXT_REQUEST_TAB_ID: AtomicU64 = AtomicU64::new(0);
static NEXT_REQUEST_TAB_GROUP_ID: AtomicU64 = AtomicU64::new(0);

/// Stable identity for one open request tab.
///
/// A tab ID is deliberately independent from a saved-request ID. This lets
/// unsaved and detached tabs retain their identity and allows a saved request
/// to be deleted without invalidating the draft that was open in the tab.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct RequestTabId(String);

impl RequestTabId {
    pub fn new() -> Self {
        Self(new_request_tab_id())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn is_valid(&self) -> bool {
        !self.0.trim().is_empty()
    }
}

impl Default for RequestTabId {
    fn default() -> Self {
        Self::new()
    }
}

/// Stable identity for a persisted tab group.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct RequestTabGroupId(String);

impl RequestTabGroupId {
    fn new() -> Self {
        Self(new_request_tab_group_id())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn is_valid(&self) -> bool {
        !self.0.trim().is_empty()
    }
}

/// Semantic tab-group color intent.
///
/// Unknown values are retained so a newer application can add colors without
/// making the persisted request-tab state unreadable by an older version.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum RequestTabGroupColor {
    Gray,
    Blue,
    Cyan,
    Green,
    Yellow,
    Orange,
    Red,
    Pink,
    #[default]
    Purple,
    Custom(String),
}

impl RequestTabGroupColor {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Gray => "gray",
            Self::Blue => "blue",
            Self::Cyan => "cyan",
            Self::Green => "green",
            Self::Yellow => "yellow",
            Self::Orange => "orange",
            Self::Red => "red",
            Self::Pink => "pink",
            Self::Purple => "purple",
            Self::Custom(value) => value,
        }
    }
}

impl Serialize for RequestTabGroupColor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RequestTabGroupColor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "gray" => Self::Gray,
            "blue" => Self::Blue,
            "cyan" => Self::Cyan,
            "green" => Self::Green,
            "yellow" => Self::Yellow,
            "orange" => Self::Orange,
            "red" => Self::Red,
            "pink" => Self::Pink,
            "purple" => Self::Purple,
            _ => Self::Custom(value),
        })
    }
}

/// Persisted presentation metadata for one tab group.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestTabGroup {
    id: RequestTabGroupId,
    title: String,
    #[serde(default)]
    color: RequestTabGroupColor,
    #[serde(default)]
    collapsed: bool,
}

impl RequestTabGroup {
    fn new(title: impl Into<String>, color: RequestTabGroupColor) -> Self {
        Self {
            id: RequestTabGroupId::new(),
            title: title.into(),
            color,
            collapsed: false,
        }
    }

    pub fn id(&self) -> &RequestTabGroupId {
        &self.id
    }

    pub fn display_title(&self) -> &str {
        if self.title.trim().is_empty() {
            DEFAULT_REQUEST_TAB_GROUP_TITLE
        } else {
            self.title.trim()
        }
    }

    pub fn color(&self) -> &RequestTabGroupColor {
        &self.color
    }

    pub fn is_collapsed(&self) -> bool {
        self.collapsed
    }
}

/// A context-menu close operation, resolved against the current tab order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestTabCloseScope {
    Current,
    Group,
}

/// Optional persisted location of a request tab.
///
/// `folder_id` and `collection_id` are useful save targets even for an
/// unsaved tab. `saved_request_id` links the tab to a durable saved request.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestTabAssociation {
    #[serde(default)]
    folder_id: Option<String>,
    #[serde(default)]
    collection_id: Option<String>,
    #[serde(default)]
    saved_request_id: Option<String>,
}

impl RequestTabAssociation {
    pub fn new(
        folder_id: Option<String>,
        collection_id: Option<String>,
        saved_request_id: Option<String>,
    ) -> Self {
        let folder_id = if collection_id.is_some() {
            folder_id
        } else {
            None
        };
        Self {
            folder_id,
            collection_id,
            saved_request_id,
        }
    }

    pub fn folder_id(&self) -> Option<&str> {
        self.folder_id.as_deref()
    }

    pub fn collection_id(&self) -> Option<&str> {
        self.collection_id.as_deref()
    }

    pub fn saved_request_id(&self) -> Option<&str> {
        self.saved_request_id.as_deref()
    }
}

/// Persistable request-editor state for one open tab.
///
/// Runtime-only response, preview, task, and cancellation state intentionally
/// live outside this model. The baseline is persisted so an unsaved or
/// detached draft retains correct dirty semantics after restoration.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestTabRecord {
    id: RequestTabId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    group_id: Option<RequestTabGroupId>,
    title: String,
    association: RequestTabAssociation,
    template: RequestTemplate,
    baseline_title: String,
    baseline_template: RequestTemplate,
    #[serde(default)]
    detached: bool,
}

impl RequestTabRecord {
    pub fn scratch() -> Self {
        Self::unsaved(
            DEFAULT_REQUEST_TAB_TITLE,
            RequestTemplate::default(),
            RequestTabAssociation::default(),
        )
    }

    pub fn unsaved(
        title: impl Into<String>,
        template: RequestTemplate,
        association: RequestTabAssociation,
    ) -> Self {
        let title = title.into();
        let template = canonical_request_template(template);
        Self {
            id: RequestTabId::new(),
            group_id: None,
            baseline_title: title.clone(),
            baseline_template: template.clone(),
            title,
            association,
            template,
            detached: false,
        }
    }

    pub fn saved(
        title: impl Into<String>,
        template: RequestTemplate,
        association: RequestTabAssociation,
    ) -> Self {
        debug_assert!(
            association.saved_request_id.is_some(),
            "a saved request tab should carry a saved-request ID"
        );
        Self::unsaved(title, template, association)
    }

    pub fn id(&self) -> &RequestTabId {
        &self.id
    }

    pub fn group_id(&self) -> Option<&RequestTabGroupId> {
        self.group_id.as_ref()
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn display_title(&self) -> &str {
        if self.title.trim().is_empty() {
            DEFAULT_REQUEST_TAB_TITLE
        } else {
            self.title.trim()
        }
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
    }

    pub fn association(&self) -> &RequestTabAssociation {
        &self.association
    }

    pub fn template(&self) -> &RequestTemplate {
        &self.template
    }

    pub fn set_template(&mut self, template: RequestTemplate) {
        self.template = canonical_request_template(template);
    }

    pub fn baseline_title(&self) -> &str {
        &self.baseline_title
    }

    pub fn baseline_template(&self) -> &RequestTemplate {
        &self.baseline_template
    }

    pub fn is_detached(&self) -> bool {
        self.detached
    }

    pub fn is_dirty(&self) -> bool {
        self.detached
            || self.title != self.baseline_title
            || self.template != self.baseline_template
    }

    /// Whether this is the canonical empty request created to preserve the
    /// non-empty tab invariant.
    pub fn is_pristine_scratch(&self) -> bool {
        self.group_id.is_none()
            && self.title == DEFAULT_REQUEST_TAB_TITLE
            && self.association == RequestTabAssociation::default()
            && self.template == canonical_request_template(RequestTemplate::default())
            && self.baseline_title == DEFAULT_REQUEST_TAB_TITLE
            && self.baseline_template == self.template
            && !self.detached
    }

    /// Establish the current title and request template as the clean baseline.
    pub fn mark_saved(&mut self, title: impl Into<String>, association: RequestTabAssociation) {
        self.title = title.into();
        self.association = association;
        self.baseline_title = self.title.clone();
        self.baseline_template = self.template.clone();
        self.detached = false;
    }

    fn detach_saved_request(&mut self, saved_request_id: &str) -> bool {
        if self.association.saved_request_id.as_deref() != Some(saved_request_id) {
            return false;
        }
        self.association.saved_request_id = None;
        self.detached = true;
        true
    }

    fn detach_collection(&mut self, collection_id: &str) -> bool {
        if self.association.collection_id.as_deref() != Some(collection_id) {
            return false;
        }
        self.association = RequestTabAssociation::default();
        self.detached = true;
        true
    }

    fn detach_folder(&mut self, collection_id: &str, folder_id: &str) -> bool {
        if self.association.collection_id.as_deref() != Some(collection_id)
            || self.association.folder_id.as_deref() != Some(folder_id)
        {
            return false;
        }
        self.association = RequestTabAssociation::new(None, Some(collection_id.to_owned()), None);
        self.detached = true;
        true
    }

    fn move_saved_request(
        &mut self,
        saved_request_id: &str,
        collection_id: &str,
        folder_id: Option<&str>,
    ) -> bool {
        if self.association.saved_request_id.as_deref() != Some(saved_request_id) {
            return false;
        }
        self.association.collection_id = Some(collection_id.to_owned());
        self.association.folder_id = folder_id.map(str::to_owned);
        true
    }

    fn rename_saved_request(&mut self, saved_request_id: &str, title: &str) -> bool {
        if self.association.saved_request_id.as_deref() != Some(saved_request_id) {
            return false;
        }
        if self.title == self.baseline_title {
            self.title = title.to_owned();
        }
        self.baseline_title = title.to_owned();
        true
    }
}

/// Result of opening a saved request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenRequestTabResult {
    pub tab_id: RequestTabId,
    pub opened: bool,
}

/// Ordered, persistable state for all open request tabs.
///
/// The fields are private so every construction and mutation path preserves
/// two invariants: there is always at least one tab, and the active ID always
/// identifies one of the contained tabs.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RequestTabs {
    tabs: Vec<RequestTabRecord>,
    active_tab_id: RequestTabId,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    groups: Vec<RequestTabGroup>,
    #[serde(default, skip_serializing_if = "is_false")]
    welcome: bool,
}

impl RequestTabs {
    pub fn new() -> Self {
        let tab = RequestTabRecord::scratch();
        let active_tab_id = tab.id.clone();
        Self {
            tabs: vec![tab],
            active_tab_id,
            groups: Vec::new(),
            welcome: false,
        }
    }

    pub fn tabs(&self) -> &[RequestTabRecord] {
        &self.tabs
    }

    /// True when the model contains only the clean scratch record used as the
    /// backing state for the Welcome surface.
    pub fn is_canonical_scratch_only(&self) -> bool {
        self.tabs.len() == 1 && self.groups.is_empty() && self.tabs[0].is_pristine_scratch()
    }

    pub fn welcome_is_open(&self) -> bool {
        self.welcome
    }

    pub fn dismiss_welcome(&mut self) {
        self.welcome = false;
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn active_tab_id(&self) -> &RequestTabId {
        &self.active_tab_id
    }

    pub fn groups(&self) -> &[RequestTabGroup] {
        &self.groups
    }

    pub fn group(&self, id: &RequestTabGroupId) -> Option<&RequestTabGroup> {
        self.groups.iter().find(|group| group.id == *id)
    }

    pub fn tabs_in_group(&self, id: &RequestTabGroupId) -> Vec<&RequestTabRecord> {
        self.tabs
            .iter()
            .filter(|tab| tab.group_id.as_ref() == Some(id))
            .collect()
    }

    /// Move a tab immediately before another visible request tab.
    ///
    /// Tabs in the same group move independently within that group. Crossing a
    /// group boundary moves only the dragged tab: it adopts the target tab's
    /// group, or becomes ungrouped when the target is ungrouped.
    pub fn reorder_tab_before(&mut self, tab_id: &RequestTabId, target_id: &RequestTabId) -> bool {
        self.reorder_tab_relative(tab_id, target_id, false)
    }

    /// Move a tab immediately after another visible request tab.
    ///
    /// Same-group moves affect the individual member. Across group boundaries,
    /// the dragged tab adopts the target tab's group membership.
    pub fn reorder_tab_after(&mut self, tab_id: &RequestTabId, target_id: &RequestTabId) -> bool {
        self.reorder_tab_relative(tab_id, target_id, true)
    }

    fn reorder_tab_relative(
        &mut self,
        tab_id: &RequestTabId,
        target_id: &RequestTabId,
        after: bool,
    ) -> bool {
        let Some(source_index) = self.tabs.iter().position(|tab| tab.id == *tab_id) else {
            return false;
        };
        let Some(target_index) = self.tabs.iter().position(|tab| tab.id == *target_id) else {
            return false;
        };
        if source_index == target_index {
            return false;
        }

        let previous_order = self
            .tabs
            .iter()
            .map(|tab| tab.id.clone())
            .collect::<Vec<_>>();
        let target_group_id = self.tabs[target_index].group_id.clone();
        let previous_group_id = self.tabs[source_index].group_id.clone();
        let mut tab = self.tabs.remove(source_index);
        tab.group_id.clone_from(&target_group_id);
        let target_index = self
            .tabs
            .iter()
            .position(|candidate| candidate.id == *target_id)
            .expect("the distinct target must remain after source removal");
        let insertion_index = target_index + usize::from(after);
        self.tabs.insert(insertion_index, tab);
        self.prune_empty_groups();

        previous_group_id != target_group_id
            || self
                .tabs
                .iter()
                .map(|tab| &tab.id)
                .ne(previous_order.iter())
    }

    /// Create a group containing `tab_id`.
    ///
    /// If the tab belonged to another group, it is moved immediately after
    /// that old group's remaining members so both groups stay contiguous.
    pub fn create_group_for_tab(
        &mut self,
        tab_id: &RequestTabId,
        title: impl Into<String>,
        color: RequestTabGroupColor,
    ) -> Option<RequestTabGroupId> {
        self.get(tab_id)?;
        let group = RequestTabGroup::new(title, color);
        let group_id = group.id.clone();
        self.groups.push(group);
        let assigned = self.set_tab_group(tab_id, Some(&group_id));
        debug_assert!(assigned, "a newly created tab group must accept its tab");
        Some(group_id)
    }

    /// Assign a tab to a group, or remove it from its current group.
    ///
    /// Group blocks are kept contiguous. Assigning inserts the tab at the end
    /// of the destination group; ungrouping places it just after its former
    /// group.
    pub fn set_tab_group(
        &mut self,
        tab_id: &RequestTabId,
        group_id: Option<&RequestTabGroupId>,
    ) -> bool {
        if let Some(group_id) = group_id
            && self.group(group_id).is_none()
        {
            return false;
        }
        let Some(index) = self.tabs.iter().position(|tab| tab.id == *tab_id) else {
            return false;
        };
        let old_group_id = self.tabs[index].group_id.clone();
        if old_group_id.as_ref() == group_id {
            return false;
        }

        let mut tab = self.tabs.remove(index);
        tab.group_id = group_id.cloned();

        let insertion_index = if let Some(group_id) = group_id {
            self.tabs
                .iter()
                .rposition(|candidate| candidate.group_id.as_ref() == Some(group_id))
                .map_or_else(
                    || {
                        old_group_id
                            .as_ref()
                            .and_then(|old_group_id| {
                                self.tabs.iter().rposition(|candidate| {
                                    candidate.group_id.as_ref() == Some(old_group_id)
                                })
                            })
                            .map_or(index.min(self.tabs.len()), |index| index + 1)
                    },
                    |index| index + 1,
                )
        } else {
            old_group_id
                .as_ref()
                .and_then(|old_group_id| {
                    self.tabs
                        .iter()
                        .rposition(|candidate| candidate.group_id.as_ref() == Some(old_group_id))
                })
                .map_or(index.min(self.tabs.len()), |index| index + 1)
        };
        self.tabs.insert(insertion_index, tab);
        self.prune_empty_groups();
        true
    }

    pub fn rename_group(&mut self, group_id: &RequestTabGroupId, title: impl Into<String>) -> bool {
        let Some(group) = self.groups.iter_mut().find(|group| group.id == *group_id) else {
            return false;
        };
        let title = title.into();
        if group.title == title {
            return false;
        }
        group.title = title;
        true
    }

    pub fn set_group_color(
        &mut self,
        group_id: &RequestTabGroupId,
        color: RequestTabGroupColor,
    ) -> bool {
        let Some(group) = self.groups.iter_mut().find(|group| group.id == *group_id) else {
            return false;
        };
        if group.color == color {
            return false;
        }
        group.color = color;
        true
    }

    pub fn set_group_collapsed(&mut self, group_id: &RequestTabGroupId, collapsed: bool) -> bool {
        let Some(group) = self.groups.iter_mut().find(|group| group.id == *group_id) else {
            return false;
        };
        if group.collapsed == collapsed {
            return false;
        }
        group.collapsed = collapsed;
        true
    }

    /// Remove a group while leaving its tabs open and contiguous.
    pub fn remove_group(&mut self, group_id: &RequestTabGroupId) -> bool {
        let Some(index) = self.groups.iter().position(|group| group.id == *group_id) else {
            return false;
        };
        self.groups.remove(index);
        for tab in &mut self.tabs {
            if tab.group_id.as_ref() == Some(group_id) {
                tab.group_id = None;
            }
        }
        true
    }

    pub fn active(&self) -> &RequestTabRecord {
        self.get(&self.active_tab_id)
            .expect("request-tab invariant violated: active tab is missing")
    }

    pub fn active_mut(&mut self) -> &mut RequestTabRecord {
        let active_tab_id = self.active_tab_id.clone();
        self.get_mut(&active_tab_id)
            .expect("request-tab invariant violated: active tab is missing")
    }

    pub fn get(&self, id: &RequestTabId) -> Option<&RequestTabRecord> {
        self.tabs.iter().find(|tab| tab.id == *id)
    }

    pub fn get_mut(&mut self, id: &RequestTabId) -> Option<&mut RequestTabRecord> {
        self.tabs.iter_mut().find(|tab| tab.id == *id)
    }

    /// Repair the persisted association of exactly one tab.
    ///
    /// Startup reconciliation uses this instead of the aggregate detach
    /// helpers because folder IDs are only collection-local state. A stale
    /// association in one tab must not detach otherwise-valid tabs that happen
    /// to reference the same folder ID.
    pub fn repair_association(
        &mut self,
        id: &RequestTabId,
        association: RequestTabAssociation,
        mark_detached: bool,
    ) -> bool {
        let Some(tab) = self.get_mut(id) else {
            return false;
        };
        let changed = tab.association != association || (mark_detached && !tab.detached);
        tab.association = association;
        if mark_detached {
            tab.detached = true;
        }
        changed
    }

    pub fn mark_tab_saved(
        &mut self,
        id: &RequestTabId,
        title_when_saved: &str,
        saved_title: impl Into<String>,
        association: RequestTabAssociation,
        persisted_template: RequestTemplate,
    ) -> bool {
        let Some(tab) = self.get_mut(id) else {
            return false;
        };
        let saved_title = saved_title.into();
        if tab.title == title_when_saved {
            tab.title = saved_title.clone();
        }
        tab.association = association;
        tab.baseline_title = saved_title;
        tab.baseline_template = canonical_request_template(persisted_template);
        tab.detached = false;
        true
    }

    pub fn activate(&mut self, id: &RequestTabId) -> bool {
        if self.get(id).is_none() {
            return false;
        }
        self.active_tab_id = id.clone();
        true
    }

    pub fn open_new(&mut self) -> RequestTabId {
        self.open_unsaved(
            DEFAULT_REQUEST_TAB_TITLE,
            RequestTemplate::default(),
            RequestTabAssociation::default(),
        )
    }

    pub fn open_new_in_group(&mut self, group_id: &RequestTabGroupId) -> Option<RequestTabId> {
        self.group(group_id)?;
        let tab_id = self.open_new();
        let assigned = self.set_tab_group(&tab_id, Some(group_id));
        debug_assert!(assigned, "an existing group must accept a new tab");
        Some(tab_id)
    }

    pub fn open_unsaved(
        &mut self,
        title: impl Into<String>,
        template: RequestTemplate,
        association: RequestTabAssociation,
    ) -> RequestTabId {
        self.welcome = false;
        let tab = RequestTabRecord::unsaved(title, template, association);
        let id = tab.id.clone();
        self.tabs.push(tab);
        self.active_tab_id = id.clone();
        id
    }

    /// Open a saved request, or activate its existing tab without replacing
    /// that tab's possibly dirty draft.
    pub fn open_saved(
        &mut self,
        title: impl Into<String>,
        template: RequestTemplate,
        association: RequestTabAssociation,
    ) -> OpenRequestTabResult {
        self.welcome = false;
        if let Some(saved_request_id) = association.saved_request_id.as_deref()
            && let Some(tab) = self
                .tabs
                .iter()
                .find(|tab| tab.association.saved_request_id.as_deref() == Some(saved_request_id))
        {
            let tab_id = tab.id.clone();
            self.active_tab_id = tab_id.clone();
            return OpenRequestTabResult {
                tab_id,
                opened: false,
            };
        }

        let tab = RequestTabRecord::saved(title, template, association);
        let tab_id = tab.id.clone();
        self.tabs.push(tab);
        self.active_tab_id = tab_id.clone();
        OpenRequestTabResult {
            tab_id,
            opened: true,
        }
    }

    /// Close a tab and return its persisted record.
    ///
    /// Closing the active tab selects the tab that shifted into its index,
    /// falling back to its previous neighbor. Closing the final tab creates
    /// and activates a fresh scratch tab.
    #[cfg(test)]
    pub fn close(&mut self, id: &RequestTabId) -> Option<RequestTabRecord> {
        self.close_tabs(std::slice::from_ref(id)).pop()
    }

    /// Resolve a context-menu close action without mutating state.
    ///
    /// IDs are returned in their current visual order so callers can inspect
    /// dirty records and present one aggregate confirmation before closing.
    pub fn close_target_ids(
        &self,
        anchor: &RequestTabId,
        scope: RequestTabCloseScope,
    ) -> Vec<RequestTabId> {
        let Some(anchor_index) = self.tabs.iter().position(|tab| tab.id == *anchor) else {
            return Vec::new();
        };
        let anchor_group_id = self.tabs[anchor_index].group_id.as_ref();

        self.tabs
            .iter()
            .enumerate()
            .filter(|(index, tab)| match scope {
                RequestTabCloseScope::Current => *index == anchor_index,
                RequestTabCloseScope::Group => {
                    anchor_group_id.is_some() && tab.group_id.as_ref() == anchor_group_id
                }
            })
            .map(|(_, tab)| tab.id.clone())
            .collect()
    }

    /// Close a set of tabs as one ordered model mutation.
    ///
    /// Unknown and duplicate IDs are ignored. If the active tab survives, it
    /// stays active. Otherwise, selection uses the first surviving tab to its
    /// right and then its previous neighbor. Closing every tab creates exactly
    /// one fresh, ungrouped scratch tab.
    pub fn close_tabs(&mut self, ids: &[RequestTabId]) -> Vec<RequestTabRecord> {
        let requested = ids.iter().collect::<HashSet<_>>();
        if requested.is_empty() {
            return Vec::new();
        }
        let removed_active_index = self
            .tabs
            .iter()
            .position(|tab| tab.id == self.active_tab_id)
            .filter(|index| requested.contains(&self.tabs[*index].id));
        let active_survives = removed_active_index.is_none();
        let fallback_id = removed_active_index.and_then(|active_index| {
            self.tabs[active_index + 1..]
                .iter()
                .find(|tab| !requested.contains(&tab.id))
                .or_else(|| {
                    self.tabs[..active_index]
                        .iter()
                        .rev()
                        .find(|tab| !requested.contains(&tab.id))
                })
                .map(|tab| tab.id.clone())
        });

        let mut retained = Vec::with_capacity(self.tabs.len());
        let mut removed = Vec::new();
        for tab in self.tabs.drain(..) {
            if requested.contains(&tab.id) {
                removed.push(tab);
            } else {
                retained.push(tab);
            }
        }
        if removed.is_empty() {
            self.tabs = retained;
            return removed;
        }

        if retained.is_empty() {
            let replacement = RequestTabRecord::scratch();
            self.active_tab_id = replacement.id.clone();
            retained.push(replacement);
            self.groups.clear();
            self.welcome = true;
        } else if !active_survives {
            self.active_tab_id =
                fallback_id.expect("a surviving tab must provide an active fallback");
            self.welcome = false;
        } else {
            self.welcome = false;
        }
        self.tabs = retained;
        self.prune_empty_groups();
        removed
    }

    fn prune_empty_groups(&mut self) {
        let used_group_ids = self
            .tabs
            .iter()
            .filter_map(|tab| tab.group_id.as_ref())
            .collect::<HashSet<_>>();
        self.groups
            .retain(|group| used_group_ids.contains(&group.id));
    }

    pub fn detach_saved_request(&mut self, saved_request_id: &str) -> usize {
        self.tabs
            .iter_mut()
            .map(|tab| usize::from(tab.detach_saved_request(saved_request_id)))
            .sum()
    }

    pub fn detach_collection(&mut self, collection_id: &str) -> usize {
        self.tabs
            .iter_mut()
            .map(|tab| usize::from(tab.detach_collection(collection_id)))
            .sum()
    }

    pub fn detach_folder(&mut self, collection_id: &str, folder_id: &str) -> usize {
        self.tabs
            .iter_mut()
            .map(|tab| usize::from(tab.detach_folder(collection_id, folder_id)))
            .sum()
    }

    pub fn move_saved_request(
        &mut self,
        saved_request_id: &str,
        collection_id: &str,
        folder_id: Option<&str>,
    ) -> usize {
        self.tabs
            .iter_mut()
            .map(|tab| {
                usize::from(tab.move_saved_request(saved_request_id, collection_id, folder_id))
            })
            .sum()
    }

    pub fn rename_saved_request(&mut self, saved_request_id: &str, title: &str) -> usize {
        self.tabs
            .iter_mut()
            .map(|tab| usize::from(tab.rename_saved_request(saved_request_id, title)))
            .sum()
    }
}

impl Default for RequestTabs {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct PersistedRequestTabs {
    #[serde(default)]
    tabs: Vec<RequestTabRecord>,
    #[serde(default)]
    active_tab_id: Option<RequestTabId>,
    #[serde(default)]
    groups: Vec<RequestTabGroup>,
    #[serde(default)]
    welcome: bool,
}

impl<'de> Deserialize<'de> for RequestTabs {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let persisted = PersistedRequestTabs::deserialize(deserializer)?;
        Ok(normalize_tabs(
            persisted.tabs,
            persisted.active_tab_id,
            persisted.groups,
            persisted.welcome,
        ))
    }
}

fn normalize_tabs(
    mut tabs: Vec<RequestTabRecord>,
    active_tab_id: Option<RequestTabId>,
    mut groups: Vec<RequestTabGroup>,
    welcome: bool,
) -> RequestTabs {
    if tabs.is_empty() {
        return RequestTabs::new();
    }

    let mut ids = HashSet::with_capacity(tabs.len());
    for tab in &mut tabs {
        tab.template = canonical_request_template(std::mem::take(&mut tab.template));
        tab.baseline_template =
            canonical_request_template(std::mem::take(&mut tab.baseline_template));
        if tab.association.collection_id.is_none() {
            tab.association.folder_id = None;
        }
        if !tab.id.is_valid() || !ids.insert(tab.id.clone()) {
            let mut replacement = RequestTabId::new();
            while !ids.insert(replacement.clone()) {
                replacement = RequestTabId::new();
            }
            tab.id = replacement;
        }
    }

    let mut group_ids = HashSet::with_capacity(groups.len());
    let mut group_id_replacements = HashMap::new();
    groups.retain_mut(|group| {
        if !group.id.is_valid() {
            let old_id = group.id.clone();
            let mut replacement = RequestTabGroupId::new();
            while group_ids.contains(&replacement) {
                replacement = RequestTabGroupId::new();
            }
            group_id_replacements
                .entry(old_id)
                .or_insert_with(|| replacement.clone());
            group.id = replacement;
        }
        group_ids.insert(group.id.clone())
    });

    for tab in &mut tabs {
        if let Some(group_id) = tab.group_id.as_mut()
            && let Some(replacement) = group_id_replacements.get(group_id)
        {
            *group_id = replacement.clone();
        }
        if tab
            .group_id
            .as_ref()
            .is_some_and(|group_id| !group_ids.contains(group_id))
        {
            tab.group_id = None;
        }
    }
    let used_group_ids = tabs
        .iter()
        .filter_map(|tab| tab.group_id.as_ref())
        .collect::<HashSet<_>>();
    groups.retain(|group| used_group_ids.contains(&group.id));
    make_group_members_contiguous(&mut tabs);

    let active_tab_id = active_tab_id
        .filter(|id| ids.contains(id))
        .unwrap_or_else(|| tabs[0].id.clone());
    let mut request_tabs = RequestTabs {
        tabs,
        active_tab_id,
        groups,
        welcome: false,
    };
    request_tabs.welcome = welcome && request_tabs.is_canonical_scratch_only();
    request_tabs
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn make_group_members_contiguous(tabs: &mut Vec<RequestTabRecord>) {
    enum OrderItem {
        Ungrouped(Box<RequestTabRecord>),
        Group(RequestTabGroupId),
    }

    let mut order = Vec::with_capacity(tabs.len());
    let mut grouped_tabs = HashMap::<RequestTabGroupId, Vec<RequestTabRecord>>::new();
    for tab in std::mem::take(tabs) {
        if let Some(group_id) = tab.group_id.clone() {
            if !grouped_tabs.contains_key(&group_id) {
                order.push(OrderItem::Group(group_id.clone()));
            }
            grouped_tabs.entry(group_id).or_default().push(tab);
        } else {
            order.push(OrderItem::Ungrouped(Box::new(tab)));
        }
    }

    for item in order {
        match item {
            OrderItem::Ungrouped(tab) => tabs.push(*tab),
            OrderItem::Group(group_id) => {
                if let Some(group) = grouped_tabs.remove(&group_id) {
                    tabs.extend(group);
                }
            }
        }
    }
}

fn canonical_request_template(mut template: RequestTemplate) -> RequestTemplate {
    template
        .request
        .headers
        .retain(|header| !header.name.trim().is_empty() || !header.value.trim().is_empty());
    template
        .request
        .body_fields
        .retain(|field| !field.name.trim().is_empty() || !field.value.trim().is_empty());
    template
}

fn new_request_tab_id() -> String {
    let created_at = Utc::now();
    let sequence = NEXT_REQUEST_TAB_ID.fetch_add(1, Ordering::Relaxed);
    format!(
        "tab-{}-{}-{sequence}",
        created_at.timestamp_micros(),
        std::process::id()
    )
}

fn new_request_tab_group_id() -> String {
    let created_at = Utc::now();
    let sequence = NEXT_REQUEST_TAB_GROUP_ID.fetch_add(1, Ordering::Relaxed);
    format!(
        "tab-group-{}-{}-{sequence}",
        created_at.timestamp_micros(),
        std::process::id()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{BodyField, HeaderEntry, RequestDraft};

    fn template(url: &str) -> RequestTemplate {
        RequestTemplate::new(RequestDraft::new("GET", url))
    }

    fn association(
        folder_id: Option<&str>,
        collection_id: Option<&str>,
        saved_request_id: Option<&str>,
    ) -> RequestTabAssociation {
        RequestTabAssociation::new(
            folder_id.map(ToOwned::to_owned),
            collection_id.map(ToOwned::to_owned),
            saved_request_id.map(ToOwned::to_owned),
        )
    }

    #[test]
    fn default_state_has_one_clean_active_scratch_tab() {
        let tabs = RequestTabs::new();

        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.active().id(), tabs.active_tab_id());
        assert_eq!(tabs.active().title(), DEFAULT_REQUEST_TAB_TITLE);
        assert!(!tabs.active().is_dirty());
        assert!(tabs.active().association().saved_request_id().is_none());
        assert!(
            association(Some("orphan-folder"), None, None)
                .folder_id()
                .is_none()
        );
    }

    #[test]
    fn placeholder_editor_rows_do_not_make_a_tab_dirty() {
        let mut request = RequestDraft::default();
        request.headers.push(HeaderEntry::new("", ""));
        request.body_fields.push(BodyField::text("", ""));
        let placeholder_template = RequestTemplate::new(request);
        let tab = RequestTabRecord::unsaved(
            DEFAULT_REQUEST_TAB_TITLE,
            placeholder_template.clone(),
            RequestTabAssociation::default(),
        );

        assert!(tab.template().request.headers.is_empty());
        assert!(tab.template().request.body_fields.is_empty());
        assert!(!tab.is_dirty());

        let mut tabs = RequestTabs::new();
        tabs.active_mut().set_template(placeholder_template);
        assert!(!tabs.active().is_dirty());
    }

    #[test]
    fn new_tabs_have_independent_ids_and_become_active() {
        let mut tabs = RequestTabs::new();
        let first = tabs.active_tab_id().clone();
        let second = tabs.open_new();
        let third = tabs.open_new();

        assert_ne!(first, second);
        assert_ne!(second, third);
        assert_eq!(tabs.active_tab_id(), &third);
        assert_eq!(tabs.len(), 3);
    }

    #[test]
    fn open_saved_deduplicates_without_overwriting_an_open_draft() {
        let mut tabs = RequestTabs::new();
        let first = tabs.open_saved(
            "Users",
            template("https://example.test/users"),
            association(Some("folder-a"), Some("collection-a"), Some("request-a")),
        );
        tabs.active_mut()
            .set_template(template("https://example.test/users?pending=true"));
        assert!(tabs.active().is_dirty());

        tabs.open_new();
        let reopened = tabs.open_saved(
            "Changed elsewhere",
            template("https://wrong.test"),
            association(Some("folder-a"), Some("collection-a"), Some("request-a")),
        );

        assert!(first.opened);
        assert!(!reopened.opened);
        assert_eq!(reopened.tab_id, first.tab_id);
        assert_eq!(tabs.active_tab_id(), &first.tab_id);
        assert_eq!(
            tabs.active().template().request.url,
            "https://example.test/users?pending=true"
        );
        assert_eq!(tabs.active().title(), "Users");
    }

    #[test]
    fn activating_unknown_id_does_not_change_active_tab() {
        let mut tabs = RequestTabs::new();
        let active = tabs.active_tab_id().clone();

        assert!(!tabs.activate(&RequestTabId("missing".to_owned())));
        assert_eq!(tabs.active_tab_id(), &active);
    }

    #[test]
    fn closing_active_tab_chooses_next_then_previous_neighbor() {
        let mut tabs = RequestTabs::new();
        let first = tabs.active_tab_id().clone();
        let second = tabs.open_new();
        let third = tabs.open_new();

        assert!(tabs.activate(&second));
        tabs.close(&second).unwrap();
        assert_eq!(tabs.active_tab_id(), &third);

        tabs.close(&third).unwrap();
        assert_eq!(tabs.active_tab_id(), &first);
    }

    #[test]
    fn closing_inactive_tab_keeps_active_and_closing_last_creates_scratch() {
        let mut tabs = RequestTabs::new();
        let first = tabs.active_tab_id().clone();
        let second = tabs.open_new();

        tabs.close(&first).unwrap();
        assert_eq!(tabs.active_tab_id(), &second);

        let closed = tabs.close(&second).unwrap();
        assert_eq!(closed.id(), &second);
        assert_eq!(tabs.len(), 1);
        assert_ne!(tabs.active_tab_id(), &second);
        assert!(!tabs.active().is_dirty());
    }

    #[test]
    fn title_and_template_changes_are_compared_to_persisted_baseline() {
        let mut tab = RequestTabRecord::saved(
            "Users",
            template("https://example.test/users"),
            association(None, Some("collection-a"), Some("request-a")),
        );
        assert!(!tab.is_dirty());

        tab.set_title("Renamed");
        assert!(tab.is_dirty());
        tab.set_title("Users");
        assert!(!tab.is_dirty());

        tab.set_template(template("https://example.test/changed"));
        assert!(tab.is_dirty());
        tab.mark_saved(
            "Users changed",
            association(None, Some("collection-a"), Some("request-a")),
        );
        assert!(!tab.is_dirty());
        assert_eq!(tab.baseline_title(), "Users changed");
        assert_eq!(tab.baseline_template(), tab.template());
    }

    #[test]
    fn detaching_saved_request_preserves_location_and_draft_but_marks_dirty() {
        let current = template("https://example.test/pending");
        let mut tabs = RequestTabs::new();
        let opened = tabs.open_saved(
            "Pending request",
            current.clone(),
            association(Some("folder-a"), Some("collection-a"), Some("request-a")),
        );

        assert_eq!(tabs.detach_saved_request("request-a"), 1);
        let tab = tabs.get(&opened.tab_id).unwrap();
        assert_eq!(tab.template(), &current);
        assert_eq!(tab.title(), "Pending request");
        assert_eq!(tab.association().folder_id(), Some("folder-a"));
        assert_eq!(tab.association().collection_id(), Some("collection-a"));
        assert!(tab.association().saved_request_id().is_none());
        assert!(tab.is_detached());
        assert!(tab.is_dirty());
        assert_eq!(tabs.detach_saved_request("request-a"), 0);
    }

    #[test]
    fn detaching_collection_unlinks_every_matching_saved_request() {
        let mut tabs = RequestTabs::new();
        let first = tabs.open_saved(
            "One",
            template("https://example.test/one"),
            association(Some("folder-a"), Some("collection-a"), Some("request-a")),
        );
        let second = tabs.open_saved(
            "Two",
            template("https://example.test/two"),
            association(Some("folder-a"), Some("collection-a"), Some("request-b")),
        );
        let unrelated = tabs.open_saved(
            "Other",
            template("https://example.test/other"),
            association(Some("folder-b"), Some("collection-b"), Some("request-c")),
        );

        assert_eq!(tabs.detach_collection("collection-a"), 2);
        for id in [&first.tab_id, &second.tab_id] {
            let tab = tabs.get(id).unwrap();
            assert_eq!(tab.association(), &RequestTabAssociation::default());
            assert!(tab.is_dirty());
        }
        assert!(!tabs.get(&unrelated.tab_id).unwrap().is_dirty());
    }

    #[test]
    fn rename_and_move_update_every_tab_linked_to_the_saved_request() {
        let mut tabs = RequestTabs::new();
        let opened = tabs.open_saved(
            "Users",
            template("https://example.test/users"),
            association(None, Some("collection-a"), Some("request-a")),
        );

        assert_eq!(
            tabs.move_saved_request("request-a", "collection-a", Some("folder-a")),
            1
        );
        assert_eq!(tabs.rename_saved_request("request-a", "All users"), 1);
        let tab = tabs.get(&opened.tab_id).unwrap();
        assert_eq!(tab.title(), "All users");
        assert_eq!(tab.baseline_title(), "All users");
        assert_eq!(tab.association().folder_id(), Some("folder-a"));
        assert!(!tab.is_dirty());
    }

    #[test]
    fn marking_an_inactive_save_preserves_edits_made_after_the_saved_snapshot() {
        let mut tabs = RequestTabs::default();
        let tab_id = tabs.active_tab_id().clone();
        let title_when_saved = tabs.active().title().to_owned();
        let persisted = template("https://example.com/saved");
        tabs.active_mut()
            .set_template(template("https://example.com/newer-edit"));
        tabs.active_mut().set_title("Newer title edit");

        assert!(tabs.mark_tab_saved(
            &tab_id,
            &title_when_saved,
            "Saved request",
            RequestTabAssociation::new(
                None,
                Some("collection-1".to_owned()),
                Some("request-1".to_owned()),
            ),
            persisted.clone(),
        ));

        let tab = tabs.active();
        assert_eq!(tab.baseline_template(), &persisted);
        assert_eq!(tab.template().request.url, "https://example.com/newer-edit");
        assert_eq!(tab.title(), "Newer title edit");
        assert!(tab.is_dirty());
        assert_eq!(tab.association().saved_request_id(), Some("request-1"));
    }

    #[test]
    fn detaching_folder_preserves_collection_root_and_unrelated_tabs() {
        let current = template("https://example.test/in-folder");
        let mut tabs = RequestTabs::new();
        let opened = tabs.open_saved(
            "In folder",
            current.clone(),
            association(Some("folder-a"), Some("collection-a"), Some("request-a")),
        );
        let unrelated = tabs.open_unsaved(
            "Other collection",
            template("https://example.test/other"),
            association(Some("folder-a"), Some("collection-b"), None),
        );

        assert_eq!(tabs.detach_folder("collection-a", "folder-a"), 1);
        let tab = tabs.get(&opened.tab_id).unwrap();
        assert_eq!(tab.template(), &current);
        assert_eq!(tab.association().collection_id(), Some("collection-a"));
        assert!(tab.association().folder_id().is_none());
        assert!(tab.association().saved_request_id().is_none());
        assert!(tab.is_dirty());
        assert_eq!(
            tabs.get(&unrelated).unwrap().association(),
            &association(Some("folder-a"), Some("collection-b"), None)
        );
    }

    #[test]
    fn serialized_state_preserves_ids_active_tab_baseline_and_detachment() {
        let mut tabs = RequestTabs::new();
        tabs.open_saved(
            "Users",
            template("https://example.test/users"),
            association(Some("folder-a"), Some("collection-a"), Some("request-a")),
        );
        tabs.active_mut()
            .set_template(template("https://example.test/users?draft=true"));
        tabs.detach_saved_request("request-a");
        let active = tabs.active_tab_id().clone();

        let json = serde_json::to_string(&tabs).unwrap();
        let restored: RequestTabs = serde_json::from_str(&json).unwrap();

        assert_eq!(restored, tabs);
        assert_eq!(restored.active_tab_id(), &active);
        assert!(restored.active().is_dirty());
        assert!(restored.active().is_detached());
    }

    #[test]
    fn deserialization_repairs_empty_missing_active_and_duplicate_ids() {
        let empty: RequestTabs =
            serde_json::from_str(r#"{"tabs":[],"active_tab_id":null}"#).unwrap();
        assert_eq!(empty.len(), 1);

        let mut first = RequestTabRecord::scratch();
        first.association.folder_id = Some("orphan-folder".to_owned());
        let first_id = first.id().clone();
        let mut duplicate = RequestTabRecord::scratch();
        duplicate.id = first_id.clone();
        let serialized = serde_json::json!({
            "tabs": [first, duplicate],
            "active_tab_id": "missing"
        });
        let repaired: RequestTabs = serde_json::from_value(serialized).unwrap();

        assert_eq!(repaired.len(), 2);
        assert_eq!(repaired.active_tab_id(), repaired.tabs()[0].id());
        assert_ne!(repaired.tabs()[0].id(), repaired.tabs()[1].id());
        assert!(repaired.tabs()[0].association().folder_id().is_none());
    }

    #[test]
    fn repairing_one_tab_association_does_not_detach_a_matching_folder_in_another_tab() {
        let mut tabs = RequestTabs::new();
        let stale = tabs.open_unsaved(
            "Stale",
            template("https://example.test/stale"),
            association(Some("shared-folder-id"), Some("collection-a"), None),
        );
        let valid = tabs.open_unsaved(
            "Valid",
            template("https://example.test/valid"),
            association(Some("shared-folder-id"), Some("collection-b"), None),
        );

        assert!(tabs.repair_association(
            &stale,
            association(None, Some("collection-a"), None),
            true,
        ));

        let stale = tabs.get(&stale).unwrap();
        assert_eq!(stale.association().collection_id(), Some("collection-a"));
        assert!(stale.association().folder_id().is_none());
        assert!(stale.is_detached());
        let valid = tabs.get(&valid).unwrap();
        assert_eq!(
            valid.association(),
            &association(Some("shared-folder-id"), Some("collection-b"), None)
        );
        assert!(!valid.is_detached());
    }

    #[test]
    fn close_target_selectors_cover_current_tab_and_request_group() {
        let mut tabs = RequestTabs::new();
        let second = tabs.open_new();
        let third = tabs.open_new();
        let group = tabs
            .create_group_for_tab(&second, "Auth", RequestTabGroupColor::Blue)
            .unwrap();
        assert!(tabs.set_tab_group(&third, Some(&group)));

        assert_eq!(
            tabs.close_target_ids(&third, RequestTabCloseScope::Current),
            vec![third.clone()]
        );
        assert_eq!(
            tabs.close_target_ids(&third, RequestTabCloseScope::Group),
            vec![second, third]
        );

        let missing = RequestTabId("missing".to_owned());
        for scope in [RequestTabCloseScope::Current, RequestTabCloseScope::Group] {
            assert!(tabs.close_target_ids(&missing, scope).is_empty());
        }
    }

    #[test]
    fn close_tabs_is_atomic_ordered_and_preserves_or_repairs_active_selection() {
        let mut tabs = RequestTabs::new();
        tabs.active_mut().set_title("First");
        let first = tabs.active_tab_id().clone();
        let second = tabs.open_new();
        tabs.active_mut().set_title("Second");
        let third = tabs.open_new();
        tabs.active_mut().set_title("Third");
        let fourth = tabs.open_new();
        tabs.active_mut().set_title("Fourth");
        let fifth = tabs.open_new();
        tabs.active_mut().set_title("Fifth");

        assert!(tabs.activate(&third));
        let removed = tabs.close_tabs(&[
            first.clone(),
            RequestTabId("missing".to_owned()),
            fifth.clone(),
            first,
        ]);
        assert_eq!(
            removed
                .iter()
                .map(RequestTabRecord::title)
                .collect::<Vec<_>>(),
            vec!["First", "Fifth"]
        );
        assert_eq!(tabs.active_tab_id(), &third);

        let removed = tabs.close_tabs(&[second, third.clone()]);
        assert_eq!(
            removed
                .iter()
                .map(RequestTabRecord::title)
                .collect::<Vec<_>>(),
            vec!["Second", "Third"]
        );
        assert_eq!(tabs.active_tab_id(), &fourth);
        assert!(
            tabs.close_tabs(&[RequestTabId("missing".to_owned())])
                .is_empty()
        );
    }

    #[test]
    fn canonical_scratch_detection_is_strict() {
        let mut tabs = RequestTabs::new();
        assert!(tabs.is_canonical_scratch_only());
        assert!(!tabs.welcome_is_open());

        tabs.active_mut().set_title("Named");
        assert!(!tabs.is_canonical_scratch_only());

        let mut tabs = RequestTabs::new();
        tabs.active_mut()
            .set_template(RequestTemplate::new(super::super::RequestDraft {
                url: "https://example.test".to_owned(),
                ..Default::default()
            }));
        assert!(!tabs.is_canonical_scratch_only());

        let mut tabs = RequestTabs::new();
        tabs.open_new();
        assert!(!tabs.is_canonical_scratch_only());
    }

    #[test]
    fn welcome_marker_is_explicit_persisted_and_cleared_by_new_work() {
        let mut tabs = RequestTabs::new();
        let initial = tabs.active_tab_id().clone();
        tabs.close(&initial).unwrap();
        assert!(tabs.welcome_is_open());
        assert!(tabs.is_canonical_scratch_only());

        let encoded = serde_json::to_string(&tabs).unwrap();
        let mut restored: RequestTabs = serde_json::from_str(&encoded).unwrap();
        assert!(restored.welcome_is_open());

        restored.dismiss_welcome();
        assert!(!restored.welcome_is_open());
        restored.open_new();
        assert!(!restored.welcome_is_open());

        let legacy: RequestTabs = serde_json::from_str(
            r#"{
                "tabs": [{
                    "id": "legacy",
                    "title": "Untitled Request",
                    "association": {},
                    "template": {},
                    "baseline_title": "Untitled Request",
                    "baseline_template": {}
                }],
                "active_tab_id": "legacy"
            }"#,
        )
        .unwrap();
        assert!(!legacy.welcome_is_open());
    }

    #[test]
    fn tabs_reorder_before_and_after_without_changing_active_identity() {
        let mut tabs = RequestTabs::new();
        let first = tabs.active_tab_id().clone();
        let second = tabs.open_new();
        let third = tabs.open_new();
        let ids = |tabs: &RequestTabs| {
            tabs.tabs()
                .iter()
                .map(|tab| tab.id().clone())
                .collect::<Vec<_>>()
        };

        assert!(tabs.reorder_tab_before(&third, &first));
        assert_eq!(ids(&tabs), [third.clone(), first.clone(), second.clone()]);
        assert_eq!(tabs.active_tab_id(), &third);

        assert!(tabs.reorder_tab_after(&third, &second));
        assert_eq!(ids(&tabs), [first.clone(), second.clone(), third.clone()]);
        assert_eq!(tabs.active_tab_id(), &third);

        assert!(!tabs.reorder_tab_after(&third, &third));
        let missing = RequestTabId("missing".to_owned());
        assert!(!tabs.reorder_tab_before(&missing, &first));
        assert!(!tabs.reorder_tab_before(&first, &missing));
        assert_eq!(ids(&tabs), [first, second, third]);

        let restored: RequestTabs = serde_json::from_str(&serde_json::to_string(&tabs).unwrap())
            .expect("reordered tabs should remain persistable");
        assert_eq!(restored, tabs);
    }

    #[test]
    fn cross_group_reorder_moves_one_tab_and_adopts_target_membership() {
        let mut tabs = RequestTabs::new();
        let first = tabs.active_tab_id().clone();
        let second = tabs.open_new();
        let third = tabs.open_new();
        let fourth = tabs.open_new();
        let fifth = tabs.open_new();
        let first_group = tabs
            .create_group_for_tab(&first, "First", RequestTabGroupColor::Green)
            .unwrap();
        assert!(tabs.set_tab_group(&second, Some(&first_group)));
        let second_group = tabs
            .create_group_for_tab(&fourth, "Second", RequestTabGroupColor::Orange)
            .unwrap();
        assert!(tabs.set_tab_group(&fifth, Some(&second_group)));

        assert!(tabs.reorder_tab_before(&second, &fourth));
        assert_eq!(tabs.get(&second).unwrap().group_id(), Some(&second_group));
        assert_eq!(tabs.tabs_in_group(&first_group).len(), 1);
        assert_eq!(
            tabs.tabs_in_group(&second_group)
                .into_iter()
                .map(|tab| tab.id().clone())
                .collect::<Vec<_>>(),
            [second.clone(), fourth.clone(), fifth.clone()]
        );

        assert!(tabs.reorder_tab_after(&first, &third));
        assert!(tabs.get(&first).unwrap().group_id().is_none());
        assert!(tabs.group(&first_group).is_none());

        assert!(tabs.reorder_tab_after(&third, &fourth));
        assert_eq!(tabs.get(&third).unwrap().group_id(), Some(&second_group));
        assert_eq!(
            tabs.tabs_in_group(&second_group)
                .into_iter()
                .map(|tab| tab.id().clone())
                .collect::<Vec<_>>(),
            [second.clone(), fourth.clone(), third.clone(), fifth.clone()]
        );

        assert!(tabs.reorder_tab_before(&fifth, &second));
        assert_eq!(
            tabs.tabs_in_group(&second_group)
                .into_iter()
                .map(|tab| tab.id().clone())
                .collect::<Vec<_>>(),
            [fifth, second, fourth, third]
        );
        assert_eq!(tabs.groups().len(), 1);

        let restored: RequestTabs = serde_json::from_str(&serde_json::to_string(&tabs).unwrap())
            .expect("group-aware reordered tabs should remain persistable");
        assert_eq!(restored, tabs);
    }

    #[test]
    fn assigning_and_ungrouping_tabs_keeps_each_group_contiguous() {
        let mut tabs = RequestTabs::new();
        let first = tabs.active_tab_id().clone();
        let second = tabs.open_new();
        let third = tabs.open_new();
        let fourth = tabs.open_new();
        let group = tabs
            .create_group_for_tab(&second, "Auth", RequestTabGroupColor::Cyan)
            .unwrap();

        assert!(tabs.set_tab_group(&fourth, Some(&group)));
        assert_eq!(
            tabs.tabs()
                .iter()
                .map(|tab| tab.id().clone())
                .collect::<Vec<_>>(),
            vec![first.clone(), second.clone(), fourth.clone(), third.clone()]
        );
        assert_eq!(tabs.tabs_in_group(&group).len(), 2);

        assert!(tabs.set_tab_group(&second, None));
        assert_eq!(
            tabs.tabs()
                .iter()
                .map(|tab| tab.id().clone())
                .collect::<Vec<_>>(),
            vec![first, fourth.clone(), second.clone(), third]
        );
        assert!(tabs.get(&second).unwrap().group_id().is_none());
        assert_eq!(tabs.tabs_in_group(&group).len(), 1);

        let new_in_group = tabs.open_new_in_group(&group).unwrap();
        let grouped_ids = tabs
            .tabs_in_group(&group)
            .into_iter()
            .map(|tab| tab.id().clone())
            .collect::<Vec<_>>();
        assert_eq!(grouped_ids, vec![fourth, new_in_group]);
    }

    #[test]
    fn closing_a_group_prunes_its_metadata_and_keeps_other_groups() {
        let mut tabs = RequestTabs::new();
        let first = tabs.active_tab_id().clone();
        let second = tabs.open_new();
        let third = tabs.open_new();
        let first_group = tabs
            .create_group_for_tab(&first, "First group", RequestTabGroupColor::Green)
            .unwrap();
        assert!(tabs.set_tab_group(&second, Some(&first_group)));
        let second_group = tabs
            .create_group_for_tab(&third, "Second group", RequestTabGroupColor::Orange)
            .unwrap();

        let target_ids = tabs.close_target_ids(&second, RequestTabCloseScope::Group);
        let removed = tabs.close_tabs(&target_ids);
        assert_eq!(removed.len(), 2);
        assert!(tabs.group(&first_group).is_none());
        assert!(tabs.group(&second_group).is_some());
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.active_tab_id(), &third);
    }

    #[test]
    fn tab_groups_round_trip_and_unknown_colors_are_preserved() {
        let mut tabs = RequestTabs::new();
        let tab_id = tabs.active_tab_id().clone();
        let group_id = tabs
            .create_group_for_tab(
                &tab_id,
                "Experimental",
                RequestTabGroupColor::Custom("ultraviolet".to_owned()),
            )
            .unwrap();
        assert!(tabs.set_group_collapsed(&group_id, true));

        let json = serde_json::to_string(&tabs).unwrap();
        let restored: RequestTabs = serde_json::from_str(&json).unwrap();

        assert_eq!(restored, tabs);
        assert_eq!(
            restored.group(&group_id).unwrap().color(),
            &RequestTabGroupColor::Custom("ultraviolet".to_owned())
        );
        assert!(restored.group(&group_id).unwrap().is_collapsed());
    }

    #[test]
    fn legacy_and_malformed_group_state_is_normalized() {
        let legacy: RequestTabs =
            serde_json::from_str(r#"{"tabs":[],"active_tab_id":null,"future_field":"ignored"}"#)
                .unwrap();
        assert_eq!(legacy.len(), 1);
        assert!(legacy.groups().is_empty());

        let mut tabs = RequestTabs::new();
        let first = tabs.active_tab_id().clone();
        let second = tabs.open_new();
        let third = tabs.open_new();
        let group = tabs
            .create_group_for_tab(&first, "Group", RequestTabGroupColor::Pink)
            .unwrap();
        assert!(tabs.set_tab_group(&third, Some(&group)));
        let mut value = serde_json::to_value(&tabs).unwrap();
        let serialized_tabs = value["tabs"].as_array_mut().unwrap();
        serialized_tabs.swap(1, 2);
        serialized_tabs[1]["group_id"] = serde_json::json!("missing-group");

        let restored: RequestTabs = serde_json::from_value(value).unwrap();
        assert_eq!(
            restored
                .tabs()
                .iter()
                .map(|tab| tab.id().clone())
                .collect::<Vec<_>>(),
            vec![first.clone(), third, second.clone()]
        );
        assert_eq!(restored.get(&first).unwrap().group_id(), Some(&group));
        assert!(restored.get(&second).unwrap().group_id().is_none());
    }
}
