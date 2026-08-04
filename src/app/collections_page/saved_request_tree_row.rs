use super::*;
use crate::app::drag_drop::{CollectionTreeDrag, CollectionTreeDropTarget, DropPlacement};

impl ApiTester {
    pub(super) fn render_saved_request_tree_row(
        &self,
        collection_index: usize,
        request_index: usize,
        depth: usize,
        drag_enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let collection = &self.workspace.collections[collection_index];
        let request = &collection.requests[request_index];
        let collection_id = collection.id.clone();
        let request_id = request.id.clone();
        let load_id = request_id.clone();
        let load_collection_id = collection_id.clone();
        let selected = self.active_saved_request_id.as_deref() == Some(&request.id);
        let method = request.definition.request.method.clone();
        let color = method_color(&method, cx);
        let row_element_id: SharedString =
            format!("saved-request-row-{}-{}", collection.id, request.id).into();
        let action_group_id: SharedString =
            format!("saved-request-actions-{}-{}", collection.id, request.id).into();
        let load_element_id: SharedString =
            format!("load-saved-request-{}-{}", collection.id, request.id).into();
        let actions_element_id: SharedString =
            format!("saved-request-menu-{}-{}", collection.id, request.id).into();
        let actions_this = cx.entity().downgrade();
        let context_this = actions_this.clone();
        let actions_collection_id = collection_id.clone();
        let context_collection_id = collection_id;
        let actions_request_id = request_id.clone();
        let context_request_id = request_id;
        let context_scope_id: SharedString = format!(
            "saved-request-context-menu-scope-{}-{}",
            collection.id, request.id
        )
        .into();
        let can_mutate = !self.sending && self.workspace_writable;
        let selected_same_collection =
            self.selected_collection_id.as_deref() == Some(collection.id.as_str());
        let move_target = selected_same_collection
            .then(|| self.selected_folder_id.clone())
            .flatten();
        let move_label = if !selected_same_collection {
            "Select a destination in this collection".to_owned()
        } else if let Some(folder_id) = move_target.as_deref() {
            let folder_name = collection
                .folder(folder_id)
                .map(|folder| folder.name.as_str())
                .unwrap_or("selected folder");
            format!("Move to “{folder_name}”")
        } else {
            "Move to collection root".to_owned()
        };
        let actions_move_label = move_label.clone();
        let context_move_label = move_label;
        let can_move = can_mutate && selected_same_collection && request.folder_id != move_target;
        let row_inset = px(14. + (depth as f32 * 14.));
        let tree_drag = CollectionTreeDrag::request(
            collection.id.clone(),
            request.id.clone(),
            request.name.clone(),
        );
        let drop_target = CollectionTreeDropTarget::Request {
            collection_id: collection.id.clone(),
            request_id: request.id.clone(),
        };
        let drop_before_target = drop_target.clone();
        let drop_after_target = drop_target;

        div()
            .id(context_scope_id)
            .w_full()
            .child(
                h_flex()
                    .id(row_element_id)
                    .relative()
                    .group(action_group_id.clone())
                    .w_full()
                    .h(px(36.))
                    .pl_2()
                    .pr_1()
                    .gap_1()
                    .rounded_sm()
                    .when(selected, |this| this.bg(cx.theme().sidebar_accent))
                    .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.62)))
                    .child(
                        h_flex()
                            .id(load_element_id)
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .gap_2()
                            .cursor_pointer()
                            .when(drag_enabled, |this| {
                                this.cursor_move()
                                    .on_drag(tree_drag, |drag, position, _, cx| {
                                        drag.preview(position, cx)
                                    })
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_saved_request_tab(
                                    load_collection_id.clone(),
                                    load_id.clone(),
                                    window,
                                    cx,
                                );
                            }))
                            .child(
                                div()
                                    .w(px(44.))
                                    .flex_shrink_0()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(color)
                                    .child(method),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_sm()
                                    .child(request.name.clone()),
                            ),
                    )
                    .child(
                        h_flex()
                            .w(px(28.))
                            .h_full()
                            .flex_shrink_0()
                            .justify_center()
                            .child(
                                Button::new(actions_element_id)
                                    .icon(IconName::EllipsisVertical)
                                    .xsmall()
                                    .ghost()
                                    .rounded_full()
                                    .tooltip("Request actions")
                                    .invisible()
                                    .group_hover(action_group_id, |style| style.visible())
                                    .dropdown_menu(move |menu, _, _| {
                                        build_saved_request_actions_menu(
                                            menu,
                                            actions_this.clone(),
                                            actions_collection_id.clone(),
                                            actions_request_id.clone(),
                                            can_mutate,
                                            actions_move_label.clone(),
                                            can_move,
                                        )
                                    }),
                            ),
                    )
                    .when(drag_enabled, |this| {
                        this.child(self.render_collection_tree_drop_zone(
                            format!("saved-request-drop-before-{}", request.id),
                            drop_before_target,
                            DropPlacement::Before,
                            px(18.),
                            cx,
                        ))
                        .child(self.render_collection_tree_drop_zone(
                            format!("saved-request-drop-after-{}", request.id),
                            drop_after_target,
                            DropPlacement::After,
                            px(18.),
                            cx,
                        ))
                    })
                    .context_menu(move |menu, _, _| {
                        build_saved_request_actions_menu(
                            menu,
                            context_this.clone(),
                            context_collection_id.clone(),
                            context_request_id.clone(),
                            can_mutate,
                            context_move_label.clone(),
                            can_move,
                        )
                    }),
            )
            .pl(row_inset)
            .into_any_element()
    }
}

fn build_saved_request_actions_menu(
    menu: PopupMenu,
    owner: gpui::WeakEntity<ApiTester>,
    collection_id: String,
    request_id: String,
    can_mutate: bool,
    move_label: String,
    can_move: bool,
) -> PopupMenu {
    let rename_this = owner.clone();
    let rename_collection_id = collection_id.clone();
    let rename_request_id = request_id.clone();
    let duplicate_this = owner.clone();
    let duplicate_collection_id = collection_id.clone();
    let duplicate_request_id = request_id.clone();
    let copy_this = owner.clone();
    let copy_collection_id = collection_id.clone();
    let copy_request_id = request_id.clone();
    let delete_this = owner.clone();
    let delete_collection_id = collection_id.clone();
    let delete_request_id = request_id.clone();
    let move_this = owner;
    let move_collection_id = collection_id;
    let move_request_id = request_id;

    menu.item(
        PopupMenuItem::new("Rename")
            .disabled(!can_mutate)
            .on_click(move |_, window, cx| {
                let rename_this = rename_this.clone();
                let rename_collection_id = rename_collection_id.clone();
                let rename_request_id = rename_request_id.clone();
                window.defer(cx, move |window, cx| {
                    if let Some(this) = rename_this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.open_saved_request_rename_dialog(
                                rename_collection_id,
                                rename_request_id,
                                window,
                                cx,
                            );
                        });
                    }
                });
            }),
    )
    .item(
        PopupMenuItem::new("Duplicate")
            .disabled(!can_mutate)
            .on_click(move |_, _, cx| {
                if let Some(this) = duplicate_this.upgrade() {
                    this.update(cx, |this, cx| {
                        this.duplicate_saved_request(
                            duplicate_collection_id.clone(),
                            duplicate_request_id.clone(),
                            cx,
                        );
                    });
                }
            }),
    )
    .item(
        PopupMenuItem::new(move_label)
            .disabled(!can_move)
            .on_click(move |_, _, cx| {
                if let Some(this) = move_this.upgrade() {
                    this.update(cx, |this, cx| {
                        this.move_saved_request_to_selected_folder(
                            move_collection_id.clone(),
                            move_request_id.clone(),
                            cx,
                        );
                    });
                }
            }),
    )
    .item(PopupMenuItem::new("Copy link").on_click(move |_, _, cx| {
        if let Some(this) = copy_this.upgrade() {
            this.update(cx, |this, cx| {
                this.copy_saved_request_link(
                    copy_collection_id.clone(),
                    copy_request_id.clone(),
                    cx,
                );
            });
        }
    }))
    .separator()
    .item(
        PopupMenuItem::new("Delete…")
            .disabled(!can_mutate)
            .on_click(move |_, window, cx| {
                let delete_this = delete_this.clone();
                let delete_collection_id = delete_collection_id.clone();
                let delete_request_id = delete_request_id.clone();
                window.defer(cx, move |window, cx| {
                    if let Some(this) = delete_this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.open_saved_request_delete_dialog(
                                delete_collection_id,
                                delete_request_id,
                                window,
                                cx,
                            );
                        });
                    }
                });
            }),
    )
}
