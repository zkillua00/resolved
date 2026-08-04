use super::*;
use crate::app::drag_drop::{CollectionTreeDrag, CollectionTreeDropTarget, DropPlacement};

impl ApiTester {
    pub(super) fn render_collection_tree_row(
        &self,
        collection_index: usize,
        expanded: bool,
        drag_enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let collection = &self.workspace.collections[collection_index];
        let collection_id = collection.id.clone();
        let selected = self.selected_collection_id.as_deref() == Some(&collection.id)
            && self.selected_folder_id.is_none();
        let renaming = self.renaming_collection_id.as_deref() == Some(&collection.id);
        let toggle_id = collection_id.clone();
        let select_id = collection_id.clone();
        let actions_id = collection_id.clone();
        let context_id = collection_id;
        let actions_this = cx.entity().downgrade();
        let context_this = actions_this.clone();
        let can_mutate = !self.sending && self.workspace_writable;
        let tree_drag =
            CollectionTreeDrag::collection(collection.id.clone(), collection.name.clone());
        let drop_target = CollectionTreeDropTarget::Collection(collection.id.clone());
        let drop_before_target = drop_target.clone();
        let drop_inside_target = drop_target.clone();
        let drop_after_target = drop_target;

        let row = h_flex()
            .id(("collection-tree-row", collection_index))
            .relative()
            .w_full()
            .h(px(38.))
            .px_1()
            .gap_1()
            .rounded_sm()
            .when(selected, |this| {
                this.bg(cx.theme().sidebar_accent.opacity(0.42))
            })
            .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.52)))
            .child(
                Button::new(("toggle-collection", collection_index))
                    .icon(if expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .xsmall()
                    .ghost()
                    .rounded_full()
                    .tooltip(if expanded {
                        "Collapse collection"
                    } else {
                        "Expand collection"
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.toggle_collection(toggle_id.clone(), window, cx);
                    })),
            )
            .child(
                gpui_component::Icon::new(if expanded {
                    IconName::FolderOpen
                } else {
                    IconName::FolderClosed
                })
                .small()
                .text_color(if selected {
                    cx.api_primary_bright()
                } else {
                    cx.theme().muted_foreground
                }),
            )
            .when(renaming, |this| {
                this.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&self.collection_name).small()),
                )
                .child(
                    Button::new(("finish-collection-rename", collection_index))
                        .icon(IconName::Check)
                        .xsmall()
                        .ghost()
                        .tooltip("Apply collection name")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.finish_collection_rename(cx);
                        })),
                )
            })
            .when(!renaming, |this| {
                this.child(
                    div()
                        .id(("select-collection", collection_index))
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_sm()
                        .font_semibold()
                        .when(drag_enabled, |this| {
                            this.cursor_move()
                                .on_drag(tree_drag, |drag, position, _, cx| {
                                    drag.preview(position, cx)
                                })
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_collection(select_id.clone(), window, cx);
                        }))
                        .child(collection.name.clone()),
                )
                .child(
                    div()
                        .px_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(collection.requests.len().to_string()),
                )
                .child(
                    Button::new(("collection-actions", collection_index))
                        .icon(IconName::EllipsisVertical)
                        .xsmall()
                        .ghost()
                        .rounded_full()
                        .disabled(!can_mutate)
                        .dropdown_menu(move |menu, _, _| {
                            build_collection_actions_menu(
                                menu,
                                actions_this.clone(),
                                actions_id.clone(),
                                can_mutate,
                            )
                        }),
                )
            })
            .when(drag_enabled && !renaming, |this| {
                this.child(self.render_collection_tree_drop_zone(
                    format!("collection-drop-before-{}", collection.id),
                    drop_before_target,
                    DropPlacement::Before,
                    px(8.),
                    cx,
                ))
                .child(self.render_collection_tree_drop_zone(
                    format!("collection-drop-inside-{}", collection.id),
                    drop_inside_target,
                    DropPlacement::Inside,
                    px(8.),
                    cx,
                ))
                .child(self.render_collection_tree_drop_zone(
                    format!("collection-drop-after-{}", collection.id),
                    drop_after_target,
                    DropPlacement::After,
                    px(8.),
                    cx,
                ))
            })
            .context_menu(move |menu, _, _| {
                build_collection_actions_menu(
                    menu,
                    context_this.clone(),
                    context_id.clone(),
                    can_mutate && !renaming,
                )
            });

        div()
            .id(("collection-context-menu-scope", collection_index))
            .w_full()
            .child(row)
            .into_any_element()
    }
}

fn build_collection_actions_menu(
    menu: PopupMenu,
    owner: gpui::WeakEntity<ApiTester>,
    collection_id: String,
    can_mutate: bool,
) -> PopupMenu {
    if !can_mutate {
        return menu;
    }

    let new_folder_this = owner.clone();
    let new_folder_id = collection_id.clone();
    let rename_this = owner.clone();
    let rename_id = collection_id.clone();
    let delete_this = owner;
    let delete_id = collection_id;

    menu.item(
        PopupMenuItem::new("New folder").on_click(move |_, window, cx| {
            if let Some(this) = new_folder_this.upgrade() {
                this.update(cx, |this, cx| {
                    this.create_collection_folder(new_folder_id.clone(), None, window, cx);
                });
            }
        }),
    )
    .separator()
    .item(PopupMenuItem::new("Rename").on_click(move |_, window, cx| {
        if let Some(this) = rename_this.upgrade() {
            this.update(cx, |this, cx| {
                this.begin_collection_rename(rename_id.clone(), window, cx);
            });
        }
    }))
    .item(
        PopupMenuItem::new("Delete…").on_click(move |_, window, cx| {
            let delete_this = delete_this.clone();
            let delete_id = delete_id.clone();
            window.defer(cx, move |window, cx| {
                if let Some(this) = delete_this.upgrade() {
                    this.update(cx, |this, cx| {
                        this.open_collection_delete_dialog(delete_id, window, cx);
                    });
                }
            });
        }),
    )
}
