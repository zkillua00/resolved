//! Positive-scope editing over the visible hierarchy. This deliberately does not
//! compress selected children into parents: a parent's own/future coverage is
//! different from the finite set of children visible at edit time.

use crate::core::{
    ProxyAssignment, ProxyScopeKind, UpstreamCollectionView, UpstreamManagementSnapshot,
};

const MAX_ASSIGNMENTS: usize = 256;
// The server identifies its built-in Owner role by ID, not editable role name.
const OWNER_ROLE_ID: &str = "00000000-0000-0000-0000-000000000001";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AssignmentCheckState {
    Unchecked,
    Checked,
    Mixed,
}

#[derive(Clone, Debug)]
pub(super) struct AssignmentNode {
    pub(super) assignment: ProxyAssignment,
    pub(super) label: String,
    pub(super) path: String,
    pub(super) children: Vec<AssignmentNode>,
    pub(super) available: bool,
    pub(super) complete: bool,
    pub(super) owner: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct AssignmentTree {
    pub(super) roots: Vec<AssignmentNode>,
}

pub(super) fn assignment_key(assignment: &ProxyAssignment) -> String {
    let kind = match assignment.scope_kind {
        ProxyScopeKind::Server => "server",
        ProxyScopeKind::Workspace => "workspace",
        ProxyScopeKind::Collection => "collection",
        ProxyScopeKind::Request => "request",
    };
    format!(
        "{kind}-{}",
        assignment.scope_id.as_deref().unwrap_or("server")
    )
}

impl AssignmentTree {
    pub(super) fn new(
        snapshot: &UpstreamManagementSnapshot,
        proxy_id: &str,
        selection: &[ProxyAssignment],
        baseline: &[ProxyAssignment],
    ) -> Self {
        let mut server = node(ProxyScopeKind::Server, None, "Server-wide", "");
        let actor_id = &snapshot.current_user.id;
        let owner = snapshot
            .current_user
            .roles
            .iter()
            .any(|role| role.id == OWNER_ROLE_ID);
        for workspace in snapshot.workspaces.as_deref().unwrap_or_default() {
            let mut root = node(
                ProxyScopeKind::Workspace,
                Some(&workspace.id),
                &workspace.name,
                &server.path,
            );
            root.complete = owner || workspace.user_ids.contains(actor_id);
            root.children = workspace
                .collections
                .iter()
                .map(|collection| collection_node(collection, &root.path, root.complete, actor_id))
                .collect();
            server.children.push(root);
        }
        let mut tree = Self {
            roots: vec![server],
        };
        // Keep baseline-only rows around so an explicit removal can be undone.
        for assignment in selection.iter().chain(baseline) {
            if tree.find_path(assignment).is_none() {
                let label = format!(
                    "Unavailable {} · {}",
                    match assignment.scope_kind {
                        ProxyScopeKind::Server => "server",
                        ProxyScopeKind::Workspace => "workspace",
                        ProxyScopeKind::Collection => "collection",
                        ProxyScopeKind::Request => "saved request",
                    },
                    assignment.scope_id.as_deref().unwrap_or("(no ID)")
                );
                tree.roots.push(AssignmentNode {
                    assignment: assignment.clone(),
                    path: label.clone(),
                    label,
                    children: Vec::new(),
                    available: false,
                    complete: false,
                    owner: None,
                });
            }
        }
        fn set_owners(
            nodes: &mut [AssignmentNode],
            snapshot: &UpstreamManagementSnapshot,
            proxy_id: &str,
        ) {
            for node in nodes {
                node.owner = snapshot
                    .proxies
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .find(|proxy| {
                        proxy.id != proxy_id && proxy.assignments.contains(&node.assignment)
                    })
                    .map(|proxy| proxy.name.clone());
                set_owners(&mut node.children, snapshot, proxy_id);
            }
        }
        set_owners(&mut tree.roots, snapshot, proxy_id);
        tree
    }

    pub(super) fn check_state(
        &self,
        assignment: &ProxyAssignment,
        selection: &[ProxyAssignment],
    ) -> AssignmentCheckState {
        let Some(path) = self.find_path(assignment) else {
            return if selection.contains(assignment) {
                AssignmentCheckState::Checked
            } else {
                AssignmentCheckState::Unchecked
            };
        };
        if path.iter().any(|node| selection.contains(&node.assignment)) {
            AssignmentCheckState::Checked
        } else if path.last().is_some_and(|node| {
            node.children
                .iter()
                .any(|child| contains_selected(child, selection))
        }) {
            AssignmentCheckState::Mixed
        } else {
            AssignmentCheckState::Unchecked
        }
    }

    pub(super) fn toggle(
        &self,
        selection: &[ProxyAssignment],
        assignment: &ProxyAssignment,
        checked: bool,
    ) -> Result<Vec<ProxyAssignment>, String> {
        let path = self.find_path(assignment).ok_or_else(|| {
            "This scope is no longer available. Reopen the assignment editor.".to_owned()
        })?;
        let target = path.last().expect("a found path is nonempty");
        let mut proposed = selection.to_vec();
        if checked {
            if self.check_state(assignment, selection) == AssignmentCheckState::Checked {
                return enforce_limit(proposed);
            }
            require_unoccupied(target)?;
            proposed.push(assignment.clone());
            // Never discard explicit descendants: they can outrank a different
            // proxy between this scope and those descendants.
            return enforce_limit(proposed);
        }

        if !target.available {
            if selection.contains(assignment) {
                proposed.retain(|value| value != assignment);
                return enforce_limit(proposed);
            }
            return Err(
                "Cannot change inherited coverage for an unavailable scope: its ancestry is unknown."
                    .into(),
            );
        }

        let ancestors = &path[..path.len() - 1];
        if let Some(start) = ancestors
            .iter()
            .position(|node| selection.contains(&node.assignment))
        {
            if ancestors[start].assignment.scope_kind == ProxyScopeKind::Server {
                return Err(
                    "Clear Server-wide first, then assign individual workspaces. A child exception cannot safely split Server-wide coverage because other workspaces may be hidden."
                        .into(),
                );
            }
            // Removing ancestor coverage requires assigning each untouched
            // sibling subtree along the path. Never push it through another
            // proxy or substitute occupied sibling scopes with their leaves.
            for index in start..path.len() - 1 {
                let parent = path[index];
                let next = path[index + 1];
                if !parent.complete {
                    return Err(format!(
                        "Cannot split {}: some children may be hidden by your access permissions. Clear that folder's assignment explicitly or ask a member with full access to make the exception.",
                        parent.path,
                    ));
                }
                require_unoccupied(parent)?;
                proposed.retain(|value| value != &parent.assignment);
                for sibling in &parent.children {
                    if sibling.assignment != next.assignment {
                        require_unoccupied(sibling)?;
                        if !proposed.contains(&sibling.assignment) {
                            proposed.push(sibling.assignment.clone());
                        }
                    }
                }
            }
        }
        // Opaque roots have no guessed ancestry, so they survive every cascade.
        proposed.retain(|value| !contains_assignment(target, value));
        enforce_limit(proposed)
    }

    fn find_path(&self, assignment: &ProxyAssignment) -> Option<Vec<&AssignmentNode>> {
        fn visit<'a>(
            node: &'a AssignmentNode,
            assignment: &ProxyAssignment,
            path: &mut Vec<&'a AssignmentNode>,
        ) -> bool {
            path.push(node);
            if node.assignment == *assignment
                || node
                    .children
                    .iter()
                    .any(|child| visit(child, assignment, path))
            {
                return true;
            }
            path.pop();
            false
        }
        let mut path = Vec::new();
        self.roots
            .iter()
            .any(|root| visit(root, assignment, &mut path))
            .then_some(path)
    }
}

fn node(kind: ProxyScopeKind, id: Option<&str>, label: &str, parent: &str) -> AssignmentNode {
    AssignmentNode {
        assignment: ProxyAssignment {
            scope_kind: kind,
            scope_id: id.map(str::to_owned),
        },
        label: label.to_owned(),
        path: if parent.is_empty() {
            label.to_owned()
        } else {
            format!("{parent} / {label}")
        },
        children: Vec::new(),
        available: true,
        complete: kind == ProxyScopeKind::Request,
        owner: None,
    }
}

fn collection_node(
    collection: &UpstreamCollectionView,
    parent: &str,
    parent_complete: bool,
    actor_id: &str,
) -> AssignmentNode {
    let mut result = node(
        ProxyScopeKind::Collection,
        Some(&collection.id),
        &collection.name,
        parent,
    );
    result.complete = parent_complete || collection.user_ids.iter().any(|id| id == actor_id);
    result.children = collection
        .sub_collections
        .iter()
        .map(|child| collection_node(child, &result.path, result.complete, actor_id))
        .chain(collection.requests.iter().map(|request| {
            node(
                ProxyScopeKind::Request,
                Some(&request.id),
                &request.name,
                &result.path,
            )
        }))
        .collect();
    result
}

fn contains_selected(node: &AssignmentNode, selection: &[ProxyAssignment]) -> bool {
    selection.contains(&node.assignment)
        || node
            .children
            .iter()
            .any(|child| contains_selected(child, selection))
}

fn contains_assignment(node: &AssignmentNode, assignment: &ProxyAssignment) -> bool {
    node.assignment == *assignment
        || node
            .children
            .iter()
            .any(|child| contains_assignment(child, assignment))
}

fn require_unoccupied(node: &AssignmentNode) -> Result<(), String> {
    match &node.owner {
        Some(owner) => Err(format!(
            "Cannot change coverage through {}: this scope belongs to proxy {owner}. Clear the broader assignment explicitly first to avoid changing routing precedence.",
            node.path,
        )),
        None => Ok(()),
    }
}

fn enforce_limit(selection: Vec<ProxyAssignment>) -> Result<Vec<ProxyAssignment>, String> {
    if selection.len() > MAX_ASSIGNMENTS {
        Err(format!(
            "This change would require {} assignments; the maximum is {MAX_ASSIGNMENTS}. No assignments were changed.",
            selection.len()
        ))
    } else {
        Ok(selection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{ManagementProxy, ManagementUser, UpstreamWorkspaceView};

    fn scope(kind: ProxyScopeKind, id: &str) -> ProxyAssignment {
        ProxyAssignment {
            scope_kind: kind,
            scope_id: (kind != ProxyScopeKind::Server).then(|| id.to_owned()),
        }
    }

    fn collection(
        id: &str,
        children: Vec<UpstreamCollectionView>,
        requests: &[&str],
    ) -> UpstreamCollectionView {
        UpstreamCollectionView {
            id: id.into(),
            workspace_id: "w".into(),
            parent_collection_id: None,
            name: id.into(),
            user_ids: vec![],
            sub_collections: children,
            requests: requests
                .iter()
                .map(|id| crate::core::UpstreamSavedRequestView {
                    id: (*id).into(),
                    collection_id: "folder".into(),
                    name: (*id).into(),
                    definition: Default::default(),
                    created_by: None,
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                })
                .collect(),
            created_by: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn snapshot() -> UpstreamManagementSnapshot {
        let now = chrono::Utc::now();
        UpstreamManagementSnapshot {
            current_user: ManagementUser {
                id: "actor".into(),
                email: "actor@example.test".into(),
                display_name: "Actor".into(),
                active: true,
                roles: vec![],
                created_by: None,
                created_at: now,
                updated_at: now,
            },
            profiles: vec![],
            users: None,
            roles: None,
            permissions: None,
            workspaces: Some(vec![UpstreamWorkspaceView {
                id: "w".into(),
                name: "Workspace".into(),
                user_ids: vec!["actor".into()],
                collections: vec![
                    collection(
                        "folder",
                        vec![collection("nested", vec![], &["a", "b"])],
                        &["direct"],
                    ),
                    collection("sibling", vec![], &["c"]),
                ],
                created_by: None,
                created_at: now,
                updated_at: now,
            }]),
            request_execution_settings: None,
            proxies: Some(vec![]),
        }
    }

    fn owner(snapshot: &mut UpstreamManagementSnapshot, assignment: ProxyAssignment) {
        snapshot.proxies.as_mut().unwrap().push(ManagementProxy {
            id: "other".into(),
            name: "Other proxy".into(),
            rules: vec![],
            assignments: vec![assignment],
            excluded_user_ids: vec![],
            excluded_role_ids: vec![],
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        });
    }

    #[test]
    fn construction_and_inherited_checks() {
        let workspace = scope(ProxyScopeKind::Workspace, "w");
        let selection = vec![workspace];
        let tree = AssignmentTree::new(&snapshot(), "ours", &selection, &selection);
        let folder = &tree.roots[0].children[0].children[0];
        assert_eq!(folder.label, "folder");
        assert_eq!(folder.path, "Server-wide / Workspace / folder");
        assert_eq!(folder.children[0].label, "nested");
        assert_eq!(folder.children[1].label, "direct");
        let request = scope(ProxyScopeKind::Request, "a");
        assert_eq!(
            tree.check_state(&request, &selection),
            AssignmentCheckState::Checked
        );
        assert_eq!(tree.toggle(&selection, &request, true).unwrap(), selection);
    }

    #[test]
    fn child_off_splits_ancestors_without_promoting_children() {
        let workspace = scope(ProxyScopeKind::Workspace, "w");
        let folder = scope(ProxyScopeKind::Collection, "folder");
        let nested = scope(ProxyScopeKind::Collection, "nested");
        let a = scope(ProxyScopeKind::Request, "a");
        let selection = vec![workspace.clone()];
        let tree = AssignmentTree::new(&snapshot(), "ours", &selection, &selection);
        let split = tree.toggle(&selection, &a, false).unwrap();
        for parent in [&workspace, &folder, &nested] {
            assert_eq!(
                tree.check_state(parent, &split),
                AssignmentCheckState::Mixed
            );
            assert!(!split.contains(parent));
        }
        for sibling in [
            scope(ProxyScopeKind::Collection, "sibling"),
            scope(ProxyScopeKind::Request, "direct"),
            scope(ProxyScopeKind::Request, "b"),
        ] {
            assert_eq!(
                tree.check_state(&sibling, &split),
                AssignmentCheckState::Checked
            );
        }
        assert_eq!(
            tree.check_state(&a, &split),
            AssignmentCheckState::Unchecked
        );
        let all_children = tree.toggle(&split, &a, true).unwrap();
        assert_eq!(
            tree.check_state(&nested, &all_children),
            AssignmentCheckState::Mixed
        );
        let restored = tree.toggle(&split, &folder, true).unwrap();
        assert_eq!(
            tree.check_state(&a, &restored),
            AssignmentCheckState::Checked
        );
        assert!(restored.contains(&scope(ProxyScopeKind::Request, "b")));
        let branch_off = tree.toggle(&restored, &folder, false).unwrap();
        assert_eq!(
            branch_off,
            vec![scope(ProxyScopeKind::Collection, "sibling")]
        );
    }

    #[test]
    fn new_direct_children_are_unchecked_but_checked_sibling_descendants_inherit() {
        let workspace = scope(ProxyScopeKind::Workspace, "w");
        let selection = vec![workspace];
        let mut snapshot = snapshot();
        let tree = AssignmentTree::new(&snapshot, "ours", &selection, &selection);
        let partial = tree
            .toggle(&selection, &scope(ProxyScopeKind::Request, "a"), false)
            .unwrap();
        let collections = &mut snapshot.workspaces.as_mut().unwrap()[0].collections;
        collections[0]
            .sub_collections
            .push(collection("new-direct", vec![], &[]));
        collections[1]
            .sub_collections
            .push(collection("new-in-sibling", vec![], &[]));
        let tree = AssignmentTree::new(&snapshot, "ours", &partial, &selection);
        assert_eq!(
            tree.check_state(&scope(ProxyScopeKind::Collection, "new-direct"), &partial),
            AssignmentCheckState::Unchecked
        );
        assert_eq!(
            tree.check_state(
                &scope(ProxyScopeKind::Collection, "new-in-sibling"),
                &partial
            ),
            AssignmentCheckState::Checked
        );
    }

    #[test]
    fn opaque_scopes_are_preserved_and_removal_can_be_undone() {
        let opaque = scope(ProxyScopeKind::Collection, "hidden");
        let selection = vec![scope(ProxyScopeKind::Workspace, "w"), opaque.clone()];
        let tree = AssignmentTree::new(&snapshot(), "ours", &selection, &selection);
        assert!(!tree.roots[1].available);
        let split = tree
            .toggle(&selection, &scope(ProxyScopeKind::Request, "a"), false)
            .unwrap();
        assert!(split.contains(&opaque));
        let removed = tree.toggle(&split, &opaque, false).unwrap();
        let rebuilt = AssignmentTree::new(&snapshot(), "ours", &removed, &selection);
        let restored = rebuilt.toggle(&removed, &opaque, true).unwrap();
        assert_eq!(restored.len(), split.len());
        assert!(split.iter().all(|value| restored.contains(value)));
        assert!(rebuilt.toggle(&removed, &opaque, false).is_err());
    }

    #[test]
    fn ownership_conflicts_are_atomic() {
        let selection = vec![scope(ProxyScopeKind::Workspace, "w")];
        for occupied in [
            scope(ProxyScopeKind::Collection, "folder"),
            scope(ProxyScopeKind::Collection, "nested"),
            scope(ProxyScopeKind::Collection, "sibling"),
            scope(ProxyScopeKind::Request, "b"),
        ] {
            let mut snapshot = snapshot();
            owner(&mut snapshot, occupied.clone());
            let tree = AssignmentTree::new(&snapshot, "ours", &selection, &selection);
            assert!(
                tree.toggle(&selection, &scope(ProxyScopeKind::Request, "a"), false)
                    .is_err()
            );
            assert!(tree.toggle(&[], &occupied, true).is_err());
            assert_eq!(selection, vec![scope(ProxyScopeKind::Workspace, "w")]);
        }
    }

    #[test]
    fn checking_parent_keeps_precedence_sensitive_descendants() {
        let mut snapshot = snapshot();
        owner(&mut snapshot, scope(ProxyScopeKind::Collection, "nested"));
        let selection = vec![scope(ProxyScopeKind::Request, "a")];
        let tree = AssignmentTree::new(&snapshot, "ours", &selection, &selection);
        let checked = tree
            .toggle(
                &selection,
                &scope(ProxyScopeKind::Collection, "folder"),
                true,
            )
            .unwrap();
        assert!(checked.contains(&selection[0]));
        assert_eq!(checked.len(), 2);
    }

    #[test]
    fn assignment_limit_includes_opaque_scopes_and_split_expansion() {
        let mut selection: Vec<_> = (0..255)
            .map(|i| scope(ProxyScopeKind::Request, &format!("opaque-{i}")))
            .collect();
        let workspace = scope(ProxyScopeKind::Workspace, "w");
        let tree = AssignmentTree::new(&snapshot(), "ours", &selection, &selection);
        selection = tree.toggle(&selection, &workspace, true).unwrap();
        assert_eq!(selection.len(), 256);
        assert!(
            tree.toggle(&selection, &scope(ProxyScopeKind::Server, ""), true)
                .is_err()
        );
        assert!(
            tree.toggle(&selection, &scope(ProxyScopeKind::Request, "a"), false)
                .is_err()
        );
        assert_eq!(
            tree.toggle(&selection, &workspace, false).unwrap().len(),
            255
        );
    }

    #[test]
    fn projected_workspace_and_collection_shells_cannot_lose_hidden_siblings() {
        let mut snapshot = snapshot();
        let workspace = &mut snapshot.workspaces.as_mut().unwrap()[0];
        workspace.user_ids.clear();
        workspace.collections[0].sub_collections[0]
            .user_ids
            .push("actor".into());
        // The actor sees this ancestor chain through the nested collection grant,
        // not because the workspace or parent collection is complete.
        for ancestor in [
            scope(ProxyScopeKind::Workspace, "w"),
            scope(ProxyScopeKind::Collection, "folder"),
        ] {
            let selection = vec![ancestor.clone()];
            let tree = AssignmentTree::new(&snapshot, "ours", &selection, &selection);
            assert!(
                tree.toggle(&selection, &scope(ProxyScopeKind::Request, "a"), false)
                    .unwrap_err()
                    .contains("children may be hidden")
            );
            assert!(
                tree.toggle(&selection, &ancestor, false)
                    .unwrap()
                    .is_empty()
            );
        }
        let selection = vec![scope(ProxyScopeKind::Collection, "nested")];
        let tree = AssignmentTree::new(&snapshot, "ours", &selection, &selection);
        assert_eq!(
            tree.toggle(&selection, &scope(ProxyScopeKind::Request, "a"), false)
                .unwrap(),
            vec![scope(ProxyScopeKind::Request, "b")],
        );
    }

    #[test]
    fn server_inheritance_cannot_be_split_even_with_no_visible_workspaces() {
        let server = scope(ProxyScopeKind::Server, "");
        let selection = vec![server.clone()];
        let tree = AssignmentTree::new(&snapshot(), "ours", &selection, &selection);
        let error = tree
            .toggle(&selection, &scope(ProxyScopeKind::Request, "a"), false)
            .unwrap_err();
        assert!(error.contains("Clear Server-wide first"));
        assert!(tree.toggle(&selection, &server, false).unwrap().is_empty());
        let mut snapshot = snapshot();
        snapshot.workspaces = None;
        let opaque = scope(ProxyScopeKind::Request, "hidden");
        let tree = AssignmentTree::new(&snapshot, "ours", &selection, &[opaque.clone()]);
        assert!(tree.toggle(&selection, &opaque, false).is_err());
        assert_eq!(tree.toggle(&[], &server, true).unwrap(), selection);
    }
}
