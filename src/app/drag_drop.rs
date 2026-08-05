use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DropPlacement {
    Before,
    Inside,
    After,
}

#[derive(Clone, Debug)]
pub(super) struct WorkspaceTabDrag {
    pub(super) tab: WorkspaceTab,
    label: SharedString,
}

impl WorkspaceTabDrag {
    pub(super) fn new(tab: WorkspaceTab, label: impl Into<SharedString>) -> Self {
        Self {
            tab,
            label: label.into(),
        }
    }

    pub(super) fn preview(&self, position: Point<Pixels>, cx: &mut App) -> Entity<DragPreview> {
        let label = self.label.clone();
        cx.new(|_| DragPreview { label, position })
    }
}

#[derive(Clone, Debug)]
pub(super) enum CollectionTreeDrag {
    Collection {
        collection_id: String,
        label: SharedString,
    },
    Folder {
        collection_id: String,
        folder_id: String,
        label: SharedString,
    },
    Request {
        collection_id: String,
        request_id: String,
        label: SharedString,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CollectionTreeDropTarget {
    Collection(String),
    Folder {
        collection_id: String,
        folder_id: String,
    },
    Request {
        collection_id: String,
        request_id: String,
    },
}

impl CollectionTreeDrag {
    pub(super) fn collection(collection_id: String, label: impl Into<SharedString>) -> Self {
        Self::Collection {
            collection_id,
            label: label.into(),
        }
    }

    pub(super) fn folder(
        collection_id: String,
        folder_id: String,
        label: impl Into<SharedString>,
    ) -> Self {
        Self::Folder {
            collection_id,
            folder_id,
            label: label.into(),
        }
    }

    pub(super) fn request(
        collection_id: String,
        request_id: String,
        label: impl Into<SharedString>,
    ) -> Self {
        Self::Request {
            collection_id,
            request_id,
            label: label.into(),
        }
    }

    pub(super) fn preview(&self, position: Point<Pixels>, cx: &mut App) -> Entity<DragPreview> {
        let label = match self {
            Self::Collection { label, .. }
            | Self::Folder { label, .. }
            | Self::Request { label, .. } => label.clone(),
        };
        cx.new(|_| DragPreview { label, position })
    }
}

pub(super) struct DragPreview {
    label: SharedString,
    position: Point<Pixels>,
}

impl Render for DragPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .pl(self.position.x - px(12.))
            .pt(self.position.y - px(12.))
            .child(
                h_flex()
                    .max_w(px(260.))
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().drag_border)
                    .bg(cx.api_surface().opacity(0.96))
                    .shadow_lg()
                    .text_sm()
                    .font_medium()
                    .text_color(cx.theme().foreground)
                    .child(self.label.clone()),
            )
    }
}

impl ApiTester {
    pub(super) fn render_collection_tree_drop_zone(
        &self,
        id: impl Into<SharedString>,
        target: CollectionTreeDropTarget,
        placement: DropPlacement,
        edge_size: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owner = cx.entity().downgrade();
        let predicate_target = target.clone();
        let drop_target = target;

        div()
            .id(id.into())
            .absolute()
            .left_0()
            .right_0()
            .when(placement == DropPlacement::Before, |this| {
                this.top_0().h(edge_size)
            })
            .when(placement == DropPlacement::Inside, |this| {
                this.top(edge_size).bottom(edge_size)
            })
            .when(placement == DropPlacement::After, |this| {
                this.bottom_0().h(edge_size)
            })
            .can_drop(move |value, _, cx| {
                let Some(drag) = value.downcast_ref::<CollectionTreeDrag>() else {
                    return false;
                };
                owner.upgrade().is_some_and(|owner| {
                    owner
                        .read(cx)
                        .can_drop_collection_tree_item(drag, &predicate_target, placement)
                })
            })
            .drag_over::<CollectionTreeDrag>(move |style, _, _, cx| match placement {
                DropPlacement::Before => style
                    .bg(cx.theme().drop_target.opacity(0.35))
                    .border_t_2()
                    .border_color(cx.theme().drag_border),
                DropPlacement::Inside => style
                    .rounded_sm()
                    .bg(cx.theme().drop_target)
                    .border_1()
                    .border_color(cx.theme().drag_border),
                DropPlacement::After => style
                    .bg(cx.theme().drop_target.opacity(0.35))
                    .border_b_2()
                    .border_color(cx.theme().drag_border),
            })
            .on_drop(cx.listener(move |this, drag: &CollectionTreeDrag, _, cx| {
                this.drop_collection_tree_item(drag.clone(), drop_target.clone(), placement, cx);
            }))
            .into_any_element()
    }

    fn can_drop_collection_tree_item(
        &self,
        drag: &CollectionTreeDrag,
        target: &CollectionTreeDropTarget,
        placement: DropPlacement,
    ) -> bool {
        if !self.workspace_writable || self.sending {
            return false;
        }

        match (drag, target) {
            (
                CollectionTreeDrag::Collection { collection_id, .. },
                CollectionTreeDropTarget::Collection(target_collection_id),
            ) => {
                placement != DropPlacement::Inside
                    && collection_id != target_collection_id
                    && self.workspace.collection(collection_id).is_some()
                    && self.workspace.collection(target_collection_id).is_some()
            }
            (
                CollectionTreeDrag::Folder {
                    collection_id,
                    folder_id,
                    ..
                },
                CollectionTreeDropTarget::Collection(target_collection_id),
            ) => {
                placement == DropPlacement::Inside
                    && collection_id == target_collection_id
                    && self
                        .workspace
                        .collection(collection_id)
                        .is_some_and(|collection| collection.folder(folder_id).is_some())
            }
            (
                CollectionTreeDrag::Folder {
                    collection_id,
                    folder_id,
                    ..
                },
                CollectionTreeDropTarget::Folder {
                    collection_id: target_collection_id,
                    folder_id: target_folder_id,
                },
            ) => {
                collection_id == target_collection_id
                    && folder_id != target_folder_id
                    && self
                        .workspace
                        .collection(collection_id)
                        .and_then(|collection| collection.folder_path_ids(target_folder_id).ok())
                        .is_some_and(|target_path| {
                            !target_path
                                .iter()
                                .any(|ancestor_id| ancestor_id == folder_id)
                        })
            }
            (
                CollectionTreeDrag::Request {
                    collection_id,
                    request_id,
                    ..
                },
                CollectionTreeDropTarget::Collection(target_collection_id),
            ) => {
                placement == DropPlacement::Inside
                    && self
                        .workspace
                        .collection(collection_id)
                        .is_some_and(|collection| {
                            collection
                                .requests
                                .iter()
                                .any(|request| request.id == *request_id)
                        })
                    && self.workspace.collection(target_collection_id).is_some()
            }
            (
                CollectionTreeDrag::Request {
                    collection_id,
                    request_id,
                    ..
                },
                CollectionTreeDropTarget::Folder {
                    collection_id: target_collection_id,
                    folder_id: target_folder_id,
                },
            ) => {
                placement == DropPlacement::Inside
                    && self
                        .workspace
                        .collection(collection_id)
                        .is_some_and(|collection| {
                            collection
                                .requests
                                .iter()
                                .any(|request| request.id == *request_id)
                        })
                    && self
                        .workspace
                        .collection(target_collection_id)
                        .is_some_and(|collection| collection.folder(target_folder_id).is_some())
            }
            (
                CollectionTreeDrag::Request {
                    collection_id,
                    request_id,
                    ..
                },
                CollectionTreeDropTarget::Request {
                    collection_id: target_collection_id,
                    request_id: target_request_id,
                },
            ) => {
                placement != DropPlacement::Inside
                    && request_id != target_request_id
                    && self
                        .workspace
                        .collection(collection_id)
                        .is_some_and(|collection| {
                            collection
                                .requests
                                .iter()
                                .any(|request| request.id == *request_id)
                        })
                    && self
                        .workspace
                        .collection(target_collection_id)
                        .is_some_and(|collection| {
                            collection
                                .requests
                                .iter()
                                .any(|request| request.id == *target_request_id)
                        })
            }
            _ => false,
        }
    }

    pub(super) fn drop_collection_tree_item(
        &mut self,
        drag: CollectionTreeDrag,
        target: CollectionTreeDropTarget,
        placement: DropPlacement,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace_writable || self.sending {
            return;
        }

        match (drag, target) {
            (
                CollectionTreeDrag::Collection { collection_id, .. },
                CollectionTreeDropTarget::Collection(target_collection_id),
            ) if placement != DropPlacement::Inside => {
                let before_collection_id = match placement {
                    DropPlacement::Before => Some(target_collection_id),
                    DropPlacement::After => {
                        self.collection_after_anchor(&collection_id, &target_collection_id)
                    }
                    DropPlacement::Inside => unreachable!(),
                };
                let mut candidate = self.workspace.clone();
                match candidate.reorder_collection(&collection_id, before_collection_id.as_deref())
                {
                    Ok(()) => {
                        if self.commit_workspace(candidate).is_ok() {
                            self.request_notice = Some("Collection order updated.".to_owned());
                        }
                    }
                    Err(error) => self.workspace_warning = Some(error.to_string()),
                }
            }
            (
                CollectionTreeDrag::Folder {
                    collection_id,
                    folder_id,
                    ..
                },
                CollectionTreeDropTarget::Collection(target_collection_id),
            ) if placement == DropPlacement::Inside && collection_id == target_collection_id => {
                self.drop_collection_folder(collection_id, folder_id, None, None, cx);
                return;
            }
            (
                CollectionTreeDrag::Folder {
                    collection_id,
                    folder_id,
                    ..
                },
                CollectionTreeDropTarget::Folder {
                    collection_id: target_collection_id,
                    folder_id: target_folder_id,
                },
            ) if collection_id == target_collection_id && folder_id != target_folder_id => {
                let Some(target_folder) = self
                    .workspace
                    .collection(&collection_id)
                    .and_then(|collection| collection.folder(&target_folder_id))
                else {
                    return;
                };
                let (parent_folder_id, before_folder_id) = match placement {
                    DropPlacement::Inside => (Some(target_folder_id), None),
                    DropPlacement::Before => (
                        target_folder.parent_folder_id.clone(),
                        Some(target_folder_id),
                    ),
                    DropPlacement::After => {
                        let parent_folder_id = target_folder.parent_folder_id.clone();
                        let before_folder_id = self.folder_after_anchor(
                            &collection_id,
                            &folder_id,
                            parent_folder_id.as_deref(),
                            &target_folder_id,
                        );
                        (parent_folder_id, before_folder_id)
                    }
                };
                self.drop_collection_folder(
                    collection_id,
                    folder_id,
                    parent_folder_id,
                    before_folder_id,
                    cx,
                );
                return;
            }
            (
                CollectionTreeDrag::Request {
                    collection_id,
                    request_id,
                    ..
                },
                CollectionTreeDropTarget::Collection(target_collection_id),
            ) if placement == DropPlacement::Inside => {
                self.drop_saved_request(
                    collection_id,
                    request_id,
                    target_collection_id,
                    None,
                    None,
                    cx,
                );
                return;
            }
            (
                CollectionTreeDrag::Request {
                    collection_id,
                    request_id,
                    ..
                },
                CollectionTreeDropTarget::Folder {
                    collection_id: target_collection_id,
                    folder_id: target_folder_id,
                },
            ) if placement == DropPlacement::Inside => {
                self.drop_saved_request(
                    collection_id,
                    request_id,
                    target_collection_id,
                    Some(target_folder_id),
                    None,
                    cx,
                );
                return;
            }
            (
                CollectionTreeDrag::Request {
                    collection_id,
                    request_id,
                    ..
                },
                CollectionTreeDropTarget::Request {
                    collection_id: target_collection_id,
                    request_id: target_request_id,
                },
            ) if request_id != target_request_id && placement != DropPlacement::Inside => {
                let Some(target_request) = self
                    .workspace
                    .collection(&target_collection_id)
                    .and_then(|collection| {
                        collection
                            .requests
                            .iter()
                            .find(|request| request.id == target_request_id)
                    })
                else {
                    return;
                };
                let target_folder_id = target_request.folder_id.clone();
                let before_request_id = match placement {
                    DropPlacement::Before => Some(target_request_id),
                    DropPlacement::After => self.request_after_anchor(
                        &collection_id,
                        &request_id,
                        &target_collection_id,
                        target_folder_id.as_deref(),
                        &target_request_id,
                    ),
                    DropPlacement::Inside => unreachable!(),
                };
                self.drop_saved_request(
                    collection_id,
                    request_id,
                    target_collection_id,
                    target_folder_id,
                    before_request_id,
                    cx,
                );
                return;
            }
            _ => return,
        }
        cx.notify();
    }

    fn drop_collection_folder(
        &mut self,
        collection_id: String,
        folder_id: String,
        parent_folder_id: Option<String>,
        before_folder_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let mut candidate = self.workspace.clone();
        match candidate.move_collection_folder_before(
            &collection_id,
            &folder_id,
            parent_folder_id.as_deref(),
            before_folder_id.as_deref(),
        ) {
            Ok(()) => {
                if self.commit_workspace(candidate).is_ok() {
                    self.selected_collection_id = Some(collection_id.clone());
                    self.selected_folder_id = Some(folder_id);
                    self.expanded_collection_ids.insert(collection_id);
                    if let Some(parent_folder_id) = parent_folder_id {
                        self.expanded_folder_ids.insert(parent_folder_id);
                    }
                    self.request_notice = Some("Folder location updated.".to_owned());
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    fn drop_saved_request(
        &mut self,
        source_collection_id: String,
        request_id: String,
        target_collection_id: String,
        target_folder_id: Option<String>,
        before_request_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let mut candidate_workspace = self.workspace.clone();
        match candidate_workspace.move_saved_request_before(
            &source_collection_id,
            &request_id,
            &target_collection_id,
            target_folder_id.as_deref(),
            before_request_id.as_deref(),
        ) {
            Ok(()) => {
                self.snapshot_active_request_tab(cx);
                let mut candidate_request_tabs = self.request_tabs.clone();
                candidate_request_tabs.move_saved_request(
                    &request_id,
                    &target_collection_id,
                    target_folder_id.as_deref(),
                );
                if self
                    .commit_workspace_and_request_tabs(candidate_workspace, candidate_request_tabs)
                    .is_ok()
                {
                    self.expanded_collection_ids
                        .insert(target_collection_id.clone());
                    if let Some(folder_id) = target_folder_id.as_ref() {
                        self.expanded_folder_ids.insert(folder_id.clone());
                    }
                    if self.active_saved_request_id.as_deref() == Some(request_id.as_str()) {
                        self.selected_collection_id = Some(target_collection_id);
                        self.selected_folder_id = target_folder_id;
                    }
                    self.sync_active_request_tab_identity();
                    self.request_notice = Some("Request location updated.".to_owned());
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
    }

    fn collection_after_anchor(
        &self,
        source_collection_id: &str,
        target_collection_id: &str,
    ) -> Option<String> {
        self.workspace
            .collections
            .iter()
            .filter(|collection| collection.id != source_collection_id)
            .skip_while(|collection| collection.id != target_collection_id)
            .nth(1)
            .map(|collection| collection.id.clone())
    }

    fn folder_after_anchor(
        &self,
        collection_id: &str,
        source_folder_id: &str,
        parent_folder_id: Option<&str>,
        target_folder_id: &str,
    ) -> Option<String> {
        self.workspace
            .collection(collection_id)?
            .folders
            .iter()
            .filter(|folder| {
                folder.id != source_folder_id
                    && folder.parent_folder_id.as_deref() == parent_folder_id
            })
            .skip_while(|folder| folder.id != target_folder_id)
            .nth(1)
            .map(|folder| folder.id.clone())
    }

    fn request_after_anchor(
        &self,
        source_collection_id: &str,
        source_request_id: &str,
        target_collection_id: &str,
        target_folder_id: Option<&str>,
        target_request_id: &str,
    ) -> Option<String> {
        self.workspace
            .collection(target_collection_id)?
            .requests
            .iter()
            .filter(|request| {
                (source_collection_id != target_collection_id || request.id != source_request_id)
                    && request.folder_id.as_deref() == target_folder_id
            })
            .skip_while(|request| request.id != target_request_id)
            .nth(1)
            .map(|request| request.id.clone())
    }
}
