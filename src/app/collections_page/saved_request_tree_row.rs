use super::*;

impl ApiTester {
    pub(super) fn render_saved_request_tree_row(
        &self,
        collection_index: usize,
        request_index: usize,
        depth: usize,
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
        let actions_collection_id = collection_id;
        let actions_request_id = request_id;
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
        let can_move = can_mutate && selected_same_collection && request.folder_id != move_target;

        h_flex()
            .id(row_element_id)
            .group(action_group_id.clone())
            .w_full()
            .h(px(42.))
            .pl(px(24. + (depth as f32 * 16.)))
            .pr_1()
            .gap_1()
            .rounded_md()
            .when(selected, |this| this.bg(cx.theme().sidebar_accent))
            .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.72)))
            .child(
                h_flex()
                    .id(load_element_id)
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .gap_2()
                    .cursor_pointer()
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
                            .w(px(52.))
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
                                let rename_this = actions_this.clone();
                                let rename_collection_id = actions_collection_id.clone();
                                let rename_request_id = actions_request_id.clone();
                                let duplicate_this = actions_this.clone();
                                let duplicate_collection_id = actions_collection_id.clone();
                                let duplicate_request_id = actions_request_id.clone();
                                let copy_this = actions_this.clone();
                                let copy_collection_id = actions_collection_id.clone();
                                let copy_request_id = actions_request_id.clone();
                                let delete_this = actions_this.clone();
                                let delete_collection_id = actions_collection_id.clone();
                                let delete_request_id = actions_request_id.clone();
                                let move_this = actions_this.clone();
                                let move_collection_id = actions_collection_id.clone();
                                let move_request_id = actions_request_id.clone();

                                menu.item(
                                    PopupMenuItem::new("Rename").disabled(!can_mutate).on_click(
                                        move |_, window, cx| {
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
                                        },
                                    ),
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
                                    PopupMenuItem::new(move_label.clone())
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
                            }),
                    ),
            )
            .into_any_element()
    }
}
