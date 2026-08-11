use super::super::*;
use super::{CollectionFolderMoveTargets, CollectionFolderRowState};
use crate::app::drag_drop::{CollectionTreeDrag, CollectionTreeDropTarget, DropPlacement};

impl ApiTester {
    pub(super) fn render_collection_folder_tree_row(
        &self,
        collection_index: usize,
        folder_index: usize,
        row_state: CollectionFolderRowState,
        drag_enabled: bool,
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
        let context_this = actions_this.clone();
        let actions_collection_id = collection_id.clone();
        let context_collection_id = collection_id;
        let actions_folder_id = folder_id.clone();
        let context_folder_id = folder_id.clone();
        let actions_parent_id = current_parent_id.clone();
        let context_parent_id = current_parent_id;
        let actions_move_targets = move_targets.clone();
        let context_move_targets = move_targets;
        let row_group: SharedString = format!("collection-folder-actions-{folder_id}").into();
        let row_id: SharedString = format!("collection-folder-tree-row-{folder_id}").into();
        let context_scope_id: SharedString =
            format!("collection-folder-context-menu-scope-{folder_id}").into();
        let toggle_id: SharedString = format!("toggle-collection-folder-{folder_id}").into();
        let select_id: SharedString = format!("select-collection-folder-{folder_id}").into();
        let finish_rename_id: SharedString = format!("finish-folder-rename-{folder_id}").into();
        let actions_id: SharedString = format!("collection-folder-actions-{folder_id}").into();
        let row_inset = px(8. + (depth as f32 * 14.));
        let can_update = !self.sending && self.can_update_collection_content();
        let can_delete = !self.sending && self.can_delete_collection_content();
        let can_create_subfolder = !self.sending && self.can_create_collection_content();
        let drag_enabled = drag_enabled && can_update;
        let tree_drag = CollectionTreeDrag::folder(
            collection.id.clone(),
            folder.id.clone(),
            folder.name.clone(),
        );
        let drop_target = CollectionTreeDropTarget::Folder {
            collection_id: collection.id.clone(),
            folder_id: folder.id.clone(),
        };
        let drop_before_target = drop_target.clone();
        let drop_inside_target = drop_target.clone();
        let drop_after_target = drop_target;
        let attribution_tooltip =
            resource_attribution_tooltip("Folder", &folder.name, folder.created_by.as_ref());

        div()
            .id(context_scope_id)
            .w_full()
            .flex()
            .child(
                h_flex()
                    .id(row_id)
                    .relative()
                    .group(row_group.clone())
                    .flex_1()
                    .h(px(36.))
                    .pl_1()
                    .pr_1()
                    .gap_1()
                    .rounded_sm()
                    .when(selected, |this| {
                        this.bg(cx.theme().sidebar_accent.opacity(0.56))
                    })
                    .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.52)))
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
                                .child(Input::new(&self.folder_name).small()),
                        )
                        .child(
                            Button::new(finish_rename_id)
                                .icon(IconName::Check)
                                .xsmall()
                                .ghost()
                                .tooltip("Apply folder name")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.finish_collection_folder_rename(window, cx);
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
                                .tooltip(move |window, cx| {
                                    Tooltip::new(attribution_tooltip.clone()).build(window, cx)
                                })
                                .when(drag_enabled, |this| {
                                    this.cursor_move()
                                        .on_drag(tree_drag, |drag, position, _, cx| {
                                            drag.preview(position, cx)
                                        })
                                })
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
                                .disabled(!can_update && !can_delete && !can_create_subfolder)
                                .dropdown_menu(move |menu, window, cx| {
                                    build_folder_actions_menu(
                                        menu,
                                        actions_this.clone(),
                                        actions_collection_id.clone(),
                                        actions_folder_id.clone(),
                                        actions_parent_id.clone(),
                                        actions_move_targets.clone(),
                                        can_create_subfolder,
                                        can_update,
                                        can_delete,
                                        window,
                                        cx,
                                    )
                                }),
                        )
                    })
                    .when(drag_enabled && !renaming, |this| {
                        this.child(self.render_collection_tree_drop_zone(
                            format!("folder-drop-before-{}", folder.id),
                            drop_before_target,
                            DropPlacement::Before,
                            px(8.),
                            cx,
                        ))
                        .child(self.render_collection_tree_drop_zone(
                            format!("folder-drop-inside-{}", folder.id),
                            drop_inside_target,
                            DropPlacement::Inside,
                            px(8.),
                            cx,
                        ))
                        .child(self.render_collection_tree_drop_zone(
                            format!("folder-drop-after-{}", folder.id),
                            drop_after_target,
                            DropPlacement::After,
                            px(8.),
                            cx,
                        ))
                    })
                    .context_menu(move |menu, window, cx| {
                        build_folder_actions_menu(
                            menu,
                            context_this.clone(),
                            context_collection_id.clone(),
                            context_folder_id.clone(),
                            context_parent_id.clone(),
                            context_move_targets.clone(),
                            can_create_subfolder,
                            can_update && !renaming,
                            can_delete,
                            window,
                            cx,
                        )
                    }),
            )
            .pl(row_inset)
            .into_any_element()
    }
}

#[allow(clippy::too_many_arguments)]
fn build_folder_actions_menu(
    menu: PopupMenu,
    owner: gpui::WeakEntity<ApiTester>,
    collection_id: String,
    folder_id: String,
    current_parent_id: Option<String>,
    move_targets: CollectionFolderMoveTargets,
    can_create_subfolder: bool,
    can_update: bool,
    can_delete: bool,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let new_this = owner.clone();
    let new_collection_id = collection_id.clone();
    let new_parent_id = folder_id.clone();
    let rename_this = owner.clone();
    let rename_collection_id = collection_id.clone();
    let rename_folder_id = folder_id.clone();
    let delete_this = owner.clone();
    let delete_collection_id = collection_id.clone();
    let delete_folder_id = folder_id.clone();
    let move_this = owner;
    let move_collection_id = collection_id;
    let move_folder_id = folder_id;

    let mut menu = menu;
    if can_create_subfolder {
        menu = menu.item(
            PopupMenuItem::new("New subfolder").on_click(move |_, window, cx| {
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
            }),
        );
    }
    if can_update {
        menu = menu
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
                for (target_parent_id, target_label) in move_targets.iter().cloned() {
                    let target_this = move_this.clone();
                    let target_collection_id = move_collection_id.clone();
                    let target_folder_id = move_folder_id.clone();
                    let is_current = current_parent_id == target_parent_id;
                    submenu = submenu.item(
                        PopupMenuItem::new(target_label)
                            .checked(is_current)
                            .disabled(is_current)
                            .on_click(move |_, window, cx| {
                                if let Some(this) = target_this.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.move_collection_folder(
                                            target_collection_id.clone(),
                                            target_folder_id.clone(),
                                            target_parent_id.clone(),
                                            window,
                                            cx,
                                        );
                                    });
                                }
                            }),
                    );
                }
                submenu
            });
    }
    if can_delete {
        if can_update {
            menu = menu.separator();
        }
        menu = menu.item(
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
        );
    }
    menu
}
