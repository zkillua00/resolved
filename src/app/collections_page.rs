use super::*;

mod collection_tree_row;
mod folder_tree_row;
mod saved_request_tree_row;

struct CollectionFolderRenderIndex {
    child_indices: HashMap<String, Vec<usize>>,
    request_indices: HashMap<String, Vec<usize>>,
    request_counts: HashMap<String, usize>,
    matching_folder_ids: BTreeSet<String>,
    move_target_index: Arc<CollectionFolderMoveTargetIndex>,
}

struct CollectionFolderMoveTargetIndex {
    targets: Vec<(Option<String>, String)>,
    ancestor_ids: HashMap<String, BTreeSet<String>>,
}

#[derive(Clone)]
struct CollectionFolderMoveTargets {
    index: Arc<CollectionFolderMoveTargetIndex>,
    folder_id: String,
}

impl CollectionFolderMoveTargets {
    fn iter(&self) -> impl Iterator<Item = &(Option<String>, String)> {
        self.index.targets.iter().filter(move |(target_id, _)| {
            target_id.as_deref().is_none_or(|target_id| {
                !self
                    .index
                    .ancestor_ids
                    .get(target_id)
                    .is_some_and(|ancestors| ancestors.contains(&self.folder_id))
            })
        })
    }
}

struct CollectionFolderRowState {
    depth: usize,
    expanded: bool,
    request_count: usize,
    move_targets: CollectionFolderMoveTargets,
}

impl CollectionFolderRenderIndex {
    fn new(collection: &Collection, query: &str) -> Self {
        let folder_indices = collection
            .folders
            .iter()
            .enumerate()
            .map(|(index, folder)| (folder.id.as_str(), index))
            .collect::<HashMap<_, _>>();
        let mut child_indices = HashMap::<String, Vec<usize>>::new();
        for (index, folder) in collection.folders.iter().enumerate() {
            child_indices
                .entry(folder.parent_folder_id.clone().unwrap_or_default())
                .or_default()
                .push(index);
        }

        let mut path_ids = HashMap::new();
        for folder in &collection.folders {
            let mut path = Vec::new();
            let mut current_id = Some(folder.id.as_str());
            let mut seen = BTreeSet::new();
            while let Some(folder_id) = current_id {
                if !seen.insert(folder_id) {
                    break;
                }
                let Some(index) = folder_indices.get(folder_id).copied() else {
                    break;
                };
                let current = &collection.folders[index];
                path.push(current.id.clone());
                current_id = current.parent_folder_id.as_deref();
            }
            path.reverse();
            path_ids.insert(folder.id.clone(), path);
        }

        let mut request_counts = HashMap::new();
        let mut request_indices = HashMap::<String, Vec<usize>>::new();
        let mut matching_folder_ids = BTreeSet::new();
        if !query.is_empty() {
            for folder in &collection.folders {
                if folder.name.to_lowercase().contains(query)
                    && let Some(path) = path_ids.get(&folder.id)
                {
                    matching_folder_ids.extend(path.iter().cloned());
                }
            }
        }
        for (request_index, request) in collection.requests.iter().enumerate() {
            request_indices
                .entry(request.folder_id.clone().unwrap_or_default())
                .or_default()
                .push(request_index);
            let Some(folder_id) = request.folder_id.as_deref() else {
                continue;
            };
            let Some(path) = path_ids.get(folder_id) else {
                continue;
            };
            for ancestor_id in path {
                *request_counts.entry(ancestor_id.clone()).or_default() += 1;
            }
            if !query.is_empty() && ApiTester::saved_request_matches_query(request, query) {
                matching_folder_ids.extend(path.iter().cloned());
            }
        }

        let targets = std::iter::once((None, "Collection root".to_owned()))
            .chain(collection.folders.iter().map(|folder| {
                let label = path_ids
                    .get(&folder.id)
                    .into_iter()
                    .flatten()
                    .filter_map(|id| folder_indices.get(id.as_str()).copied())
                    .map(|index| collection.folders[index].name.as_str())
                    .collect::<Vec<_>>()
                    .join(" / ");
                (Some(folder.id.clone()), label)
            }))
            .collect();
        let ancestor_ids = path_ids
            .into_iter()
            .map(|(folder_id, path)| (folder_id, path.into_iter().collect()))
            .collect();

        Self {
            child_indices,
            request_indices,
            request_counts,
            matching_folder_ids,
            move_target_index: Arc::new(CollectionFolderMoveTargetIndex {
                targets,
                ancestor_ids,
            }),
        }
    }

    fn children(&self, parent_folder_id: Option<&str>) -> &[usize] {
        self.child_indices
            .get(parent_folder_id.unwrap_or_default())
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    fn request_count(&self, folder_id: &str) -> usize {
        self.request_counts.get(folder_id).copied().unwrap_or(0)
    }

    fn requests(&self, folder_id: Option<&str>) -> &[usize] {
        self.request_indices
            .get(folder_id.unwrap_or_default())
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    fn move_targets(&self, folder_id: &str) -> CollectionFolderMoveTargets {
        CollectionFolderMoveTargets {
            index: Arc::clone(&self.move_target_index),
            folder_id: folder_id.to_owned(),
        }
    }
}

impl ApiTester {
    fn saved_request_matches_query(request: &SavedRequest, query: &str) -> bool {
        let draft = &request.definition.request;
        request.name.to_lowercase().contains(query)
            || draft.method.to_lowercase().contains(query)
            || draft.url.to_lowercase().contains(query)
    }

    #[allow(clippy::too_many_arguments)]
    fn append_collection_folder_rows(
        &self,
        rows: &mut Vec<AnyElement>,
        collection_index: usize,
        index: &CollectionFolderRenderIndex,
        parent_folder_id: Option<&str>,
        depth: usize,
        searching: bool,
        show_all: bool,
        drag_enabled: bool,
        query: &str,
        cx: &mut Context<Self>,
    ) {
        let collection = &self.workspace.collections[collection_index];
        for &folder_index in index.children(parent_folder_id) {
            let folder = &collection.folders[folder_index];
            if searching && !show_all && !index.matching_folder_ids.contains(&folder.id) {
                continue;
            }

            let expanded = searching || self.expanded_folder_ids.contains(folder.id.as_str());
            rows.push(self.render_collection_folder_tree_row(
                collection_index,
                folder_index,
                CollectionFolderRowState {
                    depth,
                    expanded,
                    request_count: index.request_count(&folder.id),
                    move_targets: index.move_targets(&folder.id),
                },
                drag_enabled,
                cx,
            ));
            if !expanded {
                continue;
            }

            self.append_collection_folder_rows(
                rows,
                collection_index,
                index,
                Some(&folder.id),
                depth + 1,
                searching,
                show_all,
                drag_enabled,
                query,
                cx,
            );
            for &request_index in index.requests(Some(&folder.id)) {
                let request = &collection.requests[request_index];
                if searching && !show_all && !Self::saved_request_matches_query(request, query) {
                    continue;
                }
                rows.push(self.render_saved_request_tree_row(
                    collection_index,
                    request_index,
                    depth + 1,
                    drag_enabled,
                    cx,
                ));
            }
        }
    }

    pub(super) fn render_collections(&self, cx: &mut Context<Self>) -> AnyElement {
        let query = self
            .collection_search
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        let searching = !query.is_empty();
        let drag_enabled = !searching && self.workspace_writable && !self.sending;
        let mut tree_rows = Vec::new();

        for (collection_index, collection) in self.workspace.collections.iter().enumerate() {
            let collection_matches = !searching || collection.name.to_lowercase().contains(&query);
            let request_matches = searching
                && collection
                    .requests
                    .iter()
                    .any(|request| Self::saved_request_matches_query(request, &query));
            let expanded = searching
                || self
                    .expanded_collection_ids
                    .contains(collection.id.as_str());
            let folder_index = (searching || expanded)
                .then(|| CollectionFolderRenderIndex::new(collection, &query));
            let folder_matches = folder_index
                .as_ref()
                .is_some_and(|index| !index.matching_folder_ids.is_empty());

            if searching && !collection_matches && !request_matches && !folder_matches {
                continue;
            }

            tree_rows.push(self.render_collection_tree_row(
                collection_index,
                expanded,
                drag_enabled,
                cx,
            ));

            if !expanded {
                continue;
            }
            let Some(folder_index) = folder_index.as_ref() else {
                continue;
            };

            self.append_collection_folder_rows(
                &mut tree_rows,
                collection_index,
                folder_index,
                None,
                1,
                searching,
                collection_matches,
                drag_enabled,
                &query,
                cx,
            );
            for &request_index in folder_index.requests(None) {
                let request = &collection.requests[request_index];
                if searching
                    && !collection_matches
                    && !Self::saved_request_matches_query(request, &query)
                {
                    continue;
                }
                tree_rows.push(self.render_saved_request_tree_row(
                    collection_index,
                    request_index,
                    1,
                    drag_enabled,
                    cx,
                ));
            }
        }

        v_flex()
            .size_full()
            .min_w_0()
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .bg(cx.api_surface())
            .child(
                h_flex()
                    .h(px(64.))
                    .px_3()
                    .gap_2()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(cx.theme().sidebar_border)
                    .child(
                        Button::new("create-collection")
                            .icon(IconName::Plus)
                            .small()
                            .ghost()
                            .rounded_full()
                            .tooltip("New collection")
                            .disabled(self.sending || !self.workspace_writable)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.create_collection(window, cx);
                            })),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.collection_search)
                                .prefix(IconName::Search)
                                .cleanable(true),
                        ),
                    ),
            )
            .child(
                v_flex()
                    .id("collections-scroll")
                    .flex_1()
                    .min_h_0()
                    .p_2()
                    .overflow_y_scroll()
                    .when(tree_rows.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .items_center()
                                .gap_1()
                                .px_4()
                                .py_6()
                                .text_center()
                                .text_color(cx.theme().muted_foreground)
                                .child(div().text_sm().child(if searching {
                                    "No matching collections"
                                } else {
                                    "No collections"
                                }))
                                .child(div().text_xs().child(if searching {
                                    "Try another name, method, or URL."
                                } else {
                                    "Create one to save this request."
                                })),
                        )
                    })
                    .children(tree_rows),
            )
            .when_some(self.workspace_warning.clone(), |this, warning| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(cx.theme().warning)
                        .text_xs()
                        .text_color(cx.theme().warning)
                        .child(warning),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RenderIndexFixture {
        collection: Collection,
        projects: String,
        archive: String,
        users: String,
        admin: String,
        health: String,
        legacy: String,
        root_request_one: String,
        root_request_two: String,
        users_request_one: String,
        users_request_two: String,
        projects_request: String,
    }

    fn request_template(url: &str) -> RequestTemplate {
        RequestTemplate::new(RequestDraft::new("GET", url))
    }

    fn render_index_fixture() -> RenderIndexFixture {
        let mut workspace = Workspace::default();
        let collection_id = workspace.create_collection("Service API").unwrap();
        let projects = workspace
            .create_collection_folder(&collection_id, None, "Projects")
            .unwrap();
        let archive = workspace
            .create_collection_folder(&collection_id, None, "Archive")
            .unwrap();
        let users = workspace
            .create_collection_folder(&collection_id, Some(&projects), "Users")
            .unwrap();
        let admin = workspace
            .create_collection_folder(&collection_id, Some(&users), "Admin")
            .unwrap();
        let health = workspace
            .create_collection_folder(&collection_id, Some(&projects), "Health")
            .unwrap();
        let legacy = workspace
            .create_collection_folder(&collection_id, Some(&archive), "Legacy")
            .unwrap();

        let root_request_one = workspace
            .create_saved_request_in_folder(
                &collection_id,
                None,
                "Root first",
                request_template("https://example.test/root-first"),
            )
            .unwrap();
        let users_request_one = workspace
            .create_saved_request_in_folder(
                &collection_id,
                Some(&users),
                "Users first",
                request_template("https://example.test/users-first"),
            )
            .unwrap();
        workspace
            .create_saved_request_in_folder(
                &collection_id,
                Some(&admin),
                "Admin detail",
                request_template("https://example.test/admin-detail"),
            )
            .unwrap();
        let root_request_two = workspace
            .create_saved_request_in_folder(
                &collection_id,
                None,
                "Root second",
                request_template("https://example.test/root-second"),
            )
            .unwrap();
        let projects_request = workspace
            .create_saved_request_in_folder(
                &collection_id,
                Some(&projects),
                "Projects direct",
                request_template("https://example.test/projects"),
            )
            .unwrap();
        let users_request_two = workspace
            .create_saved_request_in_folder(
                &collection_id,
                Some(&users),
                "Users second",
                request_template("https://example.test/users-second"),
            )
            .unwrap();
        workspace
            .create_saved_request_in_folder(
                &collection_id,
                Some(&health),
                "Status endpoint",
                request_template("https://example.test/health-probe-token"),
            )
            .unwrap();
        workspace
            .create_saved_request_in_folder(
                &collection_id,
                Some(&legacy),
                "Legacy endpoint",
                request_template("https://example.test/legacy"),
            )
            .unwrap();

        RenderIndexFixture {
            collection: workspace.collections.remove(0),
            projects,
            archive,
            users,
            admin,
            health,
            legacy,
            root_request_one,
            root_request_two,
            users_request_one,
            users_request_two,
            projects_request,
        }
    }

    #[test]
    fn collection_folder_render_index_preserves_child_and_request_order_and_counts_ancestors() {
        let fixture = render_index_fixture();
        let index = CollectionFolderRenderIndex::new(&fixture.collection, "");
        let folder_ids = |indices: &[usize]| {
            indices
                .iter()
                .map(|&index| fixture.collection.folders[index].id.as_str())
                .collect::<Vec<_>>()
        };
        let request_ids = |indices: &[usize]| {
            indices
                .iter()
                .map(|&index| fixture.collection.requests[index].id.as_str())
                .collect::<Vec<_>>()
        };

        assert_eq!(
            folder_ids(index.children(None)),
            vec![fixture.projects.as_str(), fixture.archive.as_str()]
        );
        assert_eq!(
            folder_ids(index.children(Some(&fixture.projects))),
            vec![fixture.users.as_str(), fixture.health.as_str()]
        );
        assert_eq!(
            folder_ids(index.children(Some(&fixture.users))),
            vec![fixture.admin.as_str()]
        );
        assert_eq!(
            request_ids(index.requests(None)),
            vec![
                fixture.root_request_one.as_str(),
                fixture.root_request_two.as_str()
            ]
        );
        assert_eq!(
            request_ids(index.requests(Some(&fixture.users))),
            vec![
                fixture.users_request_one.as_str(),
                fixture.users_request_two.as_str()
            ]
        );
        assert_eq!(
            request_ids(index.requests(Some(&fixture.projects))),
            vec![fixture.projects_request.as_str()]
        );

        assert_eq!(index.request_count(&fixture.projects), 5);
        assert_eq!(index.request_count(&fixture.users), 3);
        assert_eq!(index.request_count(&fixture.admin), 1);
        assert_eq!(index.request_count(&fixture.health), 1);
        assert_eq!(index.request_count(&fixture.archive), 1);
        assert_eq!(index.request_count(&fixture.legacy), 1);
    }

    #[test]
    fn collection_folder_render_index_search_retains_folder_and_request_ancestors() {
        let fixture = render_index_fixture();

        let folder_match = CollectionFolderRenderIndex::new(&fixture.collection, "admin");
        assert_eq!(
            folder_match.matching_folder_ids,
            BTreeSet::from([
                fixture.projects.clone(),
                fixture.users.clone(),
                fixture.admin.clone(),
            ])
        );

        let request_match =
            CollectionFolderRenderIndex::new(&fixture.collection, "health-probe-token");
        assert_eq!(
            request_match.matching_folder_ids,
            BTreeSet::from([fixture.projects.clone(), fixture.health.clone()])
        );
    }

    #[test]
    fn collection_folder_render_index_move_targets_keep_root_and_parent_but_exclude_subtree() {
        let fixture = render_index_fixture();
        let index = CollectionFolderRenderIndex::new(&fixture.collection, "");
        let users_targets = index
            .move_targets(&fixture.users)
            .iter()
            .cloned()
            .collect::<Vec<_>>();

        assert_eq!(
            users_targets,
            vec![
                (None, "Collection root".to_owned()),
                (Some(fixture.projects.clone()), "Projects".to_owned()),
                (Some(fixture.archive.clone()), "Archive".to_owned()),
                (Some(fixture.health.clone()), "Projects / Health".to_owned()),
                (Some(fixture.legacy.clone()), "Archive / Legacy".to_owned()),
            ]
        );
        assert!(
            users_targets
                .iter()
                .all(|(id, _)| id.as_deref() != Some(&fixture.users))
        );
        assert!(
            users_targets
                .iter()
                .all(|(id, _)| id.as_deref() != Some(&fixture.admin))
        );

        let archive_targets = index
            .move_targets(&fixture.archive)
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            archive_targets.contains(&(Some(fixture.admin), "Projects / Users / Admin".to_owned()))
        );
    }
}
