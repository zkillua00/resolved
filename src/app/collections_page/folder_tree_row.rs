use super::super::*;
use super::CollectionFolderRowState;

impl ApiTester {
    pub(super) fn render_collection_folder_tree_row(
        &self,
        collection_index: usize,
        folder_index: usize,
        row_state: CollectionFolderRowState,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let CollectionFolderRowState {
            depth,
            expanded,
            request_count,
            move_targets,
        } = row_state;
        let collection = &self.workspace.collections[collection_index];
        let folder = &collection.folders[folder_index];
        let collection_id = collection.id.clone();
        let folder_id = folder.id.clone();
        let current_parent_id = folder.parent_folder_id.clone();
        let selected = self.selected_collection_id.as_deref() == Some(collection.id.as_str())
            && self.selected_folder_id.as_deref() == Some(folder.id.as_str());
        let renaming = self.renaming_folder_id.as_deref() == Some(folder.id.as_str());
        let toggle_collection_id = collection_id.clone();
        let toggle_folder_id = folder_id.clone();
        let select_collection_id = collection_id.clone();
        let select_folder_id = folder_id.clone();
        let actions_this = cx.entity().downgrade();
        let row_group: SharedString = format!("collection-folder-actions-{folder_id}").into();
        let row_id: SharedString = format!("collection-folder-tree-row-{folder_id}").into();
        let toggle_id: SharedString = format!("toggle-collection-folder-{folder_id}").into();
        let select_id: SharedString = format!("select-collection-folder-{folder_id}").into();
        let finish_rename_id: SharedString = format!("finish-folder-rename-{folder_id}").into();
        let actions_id: SharedString = format!("collection-folder-actions-{folder_id}").into();
        let padding = px(18. + (depth as f32 * 16.));
        let can_mutate = !self.sending && self.workspace_writable;

        h_flex()
            .id(row_id)
            .group(row_group.clone())
            .w_full()
            .h(px(42.))
            .pl(padding)
            .pr_1()
            .gap_1()
            .rounded_md()
            .when(selected, |this| this.bg(cx.theme().sidebar_accent))
            .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.72)))
            .child(
                Button::new(toggle_id)
                    .icon(if expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .xsmall()
                    .ghost()
                    .rounded_full()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.toggle_collection_folder(
                            toggle_collection_id.clone(),
                            toggle_folder_id.clone(),
                            window,
                            cx,
                        );
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
                    primary_bright()
                } else {
                    cx.theme().muted_foreground
                }),
            )
            .when(renaming, |this| {
                this.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&self.folder_name).small()),
                )
                .child(
                    Button::new(finish_rename_id)
                        .icon(IconName::Check)
                        .xsmall()
                        .ghost()
                        .tooltip("Apply folder name")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.finish_collection_folder_rename(cx);
                        })),
                )
            })
            .when(!renaming, |this| {
                this.child(
                    div()
                        .id(select_id)
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_sm()
                        .font_medium()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_collection_folder(
                                select_collection_id.clone(),
                                select_folder_id.clone(),
                                window,
                                cx,
                            );
                        }))
                        .child(folder.name.clone()),
                )
                .child(
                    div()
                        .px_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(request_count.to_string()),
                )
                .child(
                    Button::new(actions_id)
                        .icon(IconName::EllipsisVertical)
                        .xsmall()
                        .ghost()
                        .rounded_full()
                        .invisible()
                        .group_hover(row_group, |style| style.visible())
                        .disabled(!can_mutate)
                        .dropdown_menu(move |menu, window, cx| {
                            let new_this = actions_this.clone();
                            let new_collection_id = collection_id.clone();
                            let new_parent_id = folder_id.clone();
                            let rename_this = actions_this.clone();
                            let rename_collection_id = collection_id.clone();
                            let rename_folder_id = folder_id.clone();
                            let delete_this = actions_this.clone();
                            let delete_collection_id = collection_id.clone();
                            let delete_folder_id = folder_id.clone();
                            let move_this = actions_this.clone();
                            let move_collection_id = collection_id.clone();
                            let move_folder_id = folder_id.clone();
                            let current_parent_id = current_parent_id.clone();
                            let move_targets = move_targets.clone();
                            menu.item(PopupMenuItem::new("New subfolder").on_click(
                                move |_, window, cx| {
                                    if let Some(this) = new_this.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.create_collection_folder(
                                                new_collection_id.clone(),
                                                Some(new_parent_id.clone()),
                                                window,
                                                cx,
                                            );
                                        });
                                    }
                                },
                            ))
                            .item(PopupMenuItem::new("Rename").on_click(move |_, window, cx| {
                                if let Some(this) = rename_this.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.begin_collection_folder_rename(
                                            rename_collection_id.clone(),
                                            rename_folder_id.clone(),
                                            window,
                                            cx,
                                        );
                                    });
                                }
                            }))
                            .submenu("Move", window, cx, move |mut submenu, _, _| {
                                for (target_parent_id, target_label) in move_targets.iter().cloned()
                                {
                                    let target_this = move_this.clone();
                                    let target_collection_id = move_collection_id.clone();
                                    let target_folder_id = move_folder_id.clone();
                                    let is_current = current_parent_id == target_parent_id;
                                    submenu = submenu.item(
                                        PopupMenuItem::new(target_label)
                                            .checked(is_current)
                                            .disabled(is_current)
                                            .on_click(move |_, _, cx| {
                                                if let Some(this) = target_this.upgrade() {
                                                    this.update(cx, |this, cx| {
                                                        this.move_collection_folder(
                                                            target_collection_id.clone(),
                                                            target_folder_id.clone(),
                                                            target_parent_id.clone(),
                                                            cx,
                                                        );
                                                    });
                                                }
                                            }),
                                    );
                                }
                                submenu
                            })
                            .separator()
                            .item(
                                PopupMenuItem::new("Delete…").on_click(move |_, window, cx| {
                                    let delete_this = delete_this.clone();
                                    let delete_collection_id = delete_collection_id.clone();
                                    let delete_folder_id = delete_folder_id.clone();
                                    window.defer(cx, move |window, cx| {
                                        if let Some(this) = delete_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.open_collection_folder_delete_dialog(
                                                    delete_collection_id,
                                                    delete_folder_id,
                                                    window,
                                                    cx,
                                                );
                                            });
                                        }
                                    });
                                }),
                            )
                        }),
                )
            })
            .into_any_element()
    }
}
