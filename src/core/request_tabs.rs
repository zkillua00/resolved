use std::{
    collections::HashSet,
    sync::atomic::{AtomicU64, Ordering},
};

use chrono::Utc;
use serde::{Deserialize, Deserializer, Serialize};

use super::template::RequestTemplate;

pub const DEFAULT_REQUEST_TAB_TITLE: &str = "Untitled Request";

static NEXT_REQUEST_TAB_ID: AtomicU64 = AtomicU64::new(0);

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
}

impl RequestTabs {
    pub fn new() -> Self {
        let tab = RequestTabRecord::scratch();
        let active_tab_id = tab.id.clone();
        Self {
            tabs: vec![tab],
            active_tab_id,
        }
    }

    pub fn tabs(&self) -> &[RequestTabRecord] {
        &self.tabs
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn active_tab_id(&self) -> &RequestTabId {
        &self.active_tab_id
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

    pub fn open_unsaved(
        &mut self,
        title: impl Into<String>,
        template: RequestTemplate,
        association: RequestTabAssociation,
    ) -> RequestTabId {
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
    pub fn close(&mut self, id: &RequestTabId) -> Option<RequestTabRecord> {
        let index = self.tabs.iter().position(|tab| tab.id == *id)?;
        let was_active = self.active_tab_id == *id;
        let closed = self.tabs.remove(index);

        if self.tabs.is_empty() {
            let replacement = RequestTabRecord::scratch();
            self.active_tab_id = replacement.id.clone();
            self.tabs.push(replacement);
        } else if was_active {
            let next_index = index.min(self.tabs.len() - 1);
            self.active_tab_id = self.tabs[next_index].id.clone();
        }

        Some(closed)
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
}

impl<'de> Deserialize<'de> for RequestTabs {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let persisted = PersistedRequestTabs::deserialize(deserializer)?;
        Ok(normalize_tabs(persisted.tabs, persisted.active_tab_id))
    }
}

fn normalize_tabs(
    mut tabs: Vec<RequestTabRecord>,
    active_tab_id: Option<RequestTabId>,
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

    let active_tab_id = active_tab_id
        .filter(|id| ids.contains(id))
        .unwrap_or_else(|| tabs[0].id.clone());
    RequestTabs {
        tabs,
        active_tab_id,
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
}
