use crate::core::{RequestTabAssociation, RequestTabs, Workspace};

/// Reconcile restored tab locations against a workspace that was loaded safely.
///
/// The trust gate is part of this pure helper so startup cannot accidentally
/// repair associations against a fallback workspace after a load failure.
pub(super) fn reconcile_restored_request_tabs(
    request_tabs: &mut RequestTabs,
    workspace: &Workspace,
    workspace_trusted: bool,
) -> bool {
    if !workspace_trusted {
        return false;
    }

    let restored_tabs = request_tabs
        .tabs()
        .iter()
        .map(|tab| (tab.id().clone(), tab.association().clone()))
        .collect::<Vec<_>>();
    let mut changed = false;

    for (tab_id, association) in restored_tabs {
        let mut repaired = association.clone();
        let mut mark_detached = false;

        if let Some(request_id) = association.saved_request_id() {
            if let Some((collection, request)) = workspace.saved_request(request_id) {
                repaired = RequestTabAssociation::new(
                    request.folder_id.clone(),
                    Some(collection.id.clone()),
                    Some(request_id.to_owned()),
                );
            } else {
                repaired = RequestTabAssociation::new(
                    association.folder_id().map(ToOwned::to_owned),
                    association.collection_id().map(ToOwned::to_owned),
                    None,
                );
                mark_detached = true;
            }
        }

        match repaired.collection_id() {
            None => {
                if repaired.folder_id().is_some() {
                    repaired = RequestTabAssociation::default();
                    mark_detached = true;
                }
            }
            Some(collection_id) => match workspace.collection(collection_id) {
                None => {
                    repaired = RequestTabAssociation::default();
                    mark_detached = true;
                }
                Some(collection)
                    if repaired
                        .folder_id()
                        .is_some_and(|folder_id| collection.folder(folder_id).is_none()) =>
                {
                    repaired =
                        RequestTabAssociation::new(None, Some(collection_id.to_owned()), None);
                    mark_detached = true;
                }
                Some(_) => {}
            },
        }

        changed |= request_tabs.repair_association(&tab_id, repaired, mark_detached);
    }

    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{RequestDraft, RequestTemplate};

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
    fn valid_saved_request_is_relocated_to_its_actual_collection_and_folder() {
        let mut workspace = Workspace::default();
        let actual_collection = workspace.create_collection("Actual").unwrap();
        let actual_folder = workspace
            .create_collection_folder(&actual_collection, None, "Actual folder")
            .unwrap();
        let request_id = workspace
            .create_saved_request_in_folder(
                &actual_collection,
                Some(&actual_folder),
                "Saved request",
                template("https://example.test/saved"),
            )
            .unwrap();
        let stale_collection = workspace.create_collection("Stale").unwrap();
        let stale_folder = workspace
            .create_collection_folder(&stale_collection, None, "Stale folder")
            .unwrap();

        let mut tabs = RequestTabs::new();
        let opened = tabs.open_saved(
            "Draft",
            template("https://example.test/draft"),
            association(
                Some(&stale_folder),
                Some(&stale_collection),
                Some(&request_id),
            ),
        );

        assert!(reconcile_restored_request_tabs(&mut tabs, &workspace, true));
        let tab = tabs.get(&opened.tab_id).unwrap();
        assert_eq!(
            tab.association(),
            &association(
                Some(&actual_folder),
                Some(&actual_collection),
                Some(&request_id)
            )
        );
        assert_eq!(tab.template().request.url, "https://example.test/draft");
        assert!(!tab.is_detached());
    }

    #[test]
    fn missing_saved_request_retains_valid_location_and_becomes_detached() {
        let mut workspace = Workspace::default();
        let collection_id = workspace.create_collection("Collection").unwrap();
        let folder_id = workspace
            .create_collection_folder(&collection_id, None, "Folder")
            .unwrap();
        let mut tabs = RequestTabs::new();
        let opened = tabs.open_saved(
            "Missing",
            template("https://example.test/missing"),
            association(
                Some(&folder_id),
                Some(&collection_id),
                Some("missing-request"),
            ),
        );

        assert!(reconcile_restored_request_tabs(&mut tabs, &workspace, true));
        let tab = tabs.get(&opened.tab_id).unwrap();
        assert_eq!(
            tab.association(),
            &association(Some(&folder_id), Some(&collection_id), None)
        );
        assert!(tab.is_detached());
        assert!(tab.is_dirty());
    }

    #[test]
    fn missing_collection_fully_detaches_the_tab() {
        let workspace = Workspace::default();
        let mut tabs = RequestTabs::new();
        let tab_id = tabs.open_unsaved(
            "Missing collection",
            template("https://example.test/missing-collection"),
            association(Some("missing-folder"), Some("missing-collection"), None),
        );

        assert!(reconcile_restored_request_tabs(&mut tabs, &workspace, true));
        let tab = tabs.get(&tab_id).unwrap();
        assert_eq!(tab.association(), &RequestTabAssociation::default());
        assert!(tab.is_detached());
    }

    #[test]
    fn missing_folder_preserves_the_collection_root() {
        let mut workspace = Workspace::default();
        let collection_id = workspace.create_collection("Collection").unwrap();
        let mut tabs = RequestTabs::new();
        let tab_id = tabs.open_unsaved(
            "Missing folder",
            template("https://example.test/missing-folder"),
            association(Some("missing-folder"), Some(&collection_id), None),
        );

        assert!(reconcile_restored_request_tabs(&mut tabs, &workspace, true));
        let tab = tabs.get(&tab_id).unwrap();
        assert_eq!(
            tab.association(),
            &association(None, Some(&collection_id), None)
        );
        assert!(tab.is_detached());
    }

    #[test]
    fn matching_folder_ids_in_different_collections_only_repair_the_stale_tab() {
        let mut workspace = Workspace::default();
        let stale_collection = workspace.create_collection("Stale").unwrap();
        let valid_collection = workspace.create_collection("Valid").unwrap();
        let shared_folder_id = workspace
            .create_collection_folder(&valid_collection, None, "Shared ID")
            .unwrap();
        let mut tabs = RequestTabs::new();
        let stale_tab_id = tabs.open_unsaved(
            "Stale",
            template("https://example.test/stale"),
            association(Some(&shared_folder_id), Some(&stale_collection), None),
        );
        let valid_tab_id = tabs.open_unsaved(
            "Valid",
            template("https://example.test/valid"),
            association(Some(&shared_folder_id), Some(&valid_collection), None),
        );

        assert!(reconcile_restored_request_tabs(&mut tabs, &workspace, true));
        let stale = tabs.get(&stale_tab_id).unwrap();
        assert_eq!(
            stale.association(),
            &association(None, Some(&stale_collection), None)
        );
        assert!(stale.is_detached());
        let valid = tabs.get(&valid_tab_id).unwrap();
        assert_eq!(
            valid.association(),
            &association(Some(&shared_folder_id), Some(&valid_collection), None)
        );
        assert!(!valid.is_detached());
    }

    #[test]
    fn untrusted_workspace_does_not_reconcile_or_mutate_tabs() {
        let workspace = Workspace::default();
        let mut tabs = RequestTabs::new();
        tabs.open_saved(
            "Preserved",
            template("https://example.test/preserved"),
            association(
                Some("untrusted-folder"),
                Some("untrusted-collection"),
                Some("untrusted-request"),
            ),
        );
        let original = tabs.clone();

        assert!(!reconcile_restored_request_tabs(
            &mut tabs, &workspace, false
        ));
        assert_eq!(tabs, original);
    }
}
