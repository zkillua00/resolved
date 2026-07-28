use super::*;

impl ApiTester {
    pub(super) fn render_collection_tree_row(
        &self,
        collection_index: usize,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let collection = &self.workspace.collections[collection_index];
        let collection_id = collection.id.clone();
        let selected = self.selected_collection_id.as_deref() == Some(&collection.id);
        let renaming = self.renaming_collection_id.as_deref() == Some(&collection.id);
        let toggle_id = collection_id.clone();
        let select_id = collection_id.clone();
        let rename_id = collection_id.clone();
        let delete_id = collection_id;
        let this = cx.entity().downgrade();
        let rename_this = this.clone();
        let delete_this = this;

        h_flex()
            .id(("collection-tree-row", collection_index))
            .w_full()
            .h(px(44.))
            .px_1()
            .gap_1()
            .rounded_md()
            .when(selected, |this| this.bg(cx.theme().sidebar_accent))
            .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.72)))
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
                        .disabled(self.sending || !self.workspace_writable)
                        .dropdown_menu(move |menu, _, _| {
                            let rename_id = rename_id.clone();
                            let rename_this = rename_this.clone();
                            let delete_id = delete_id.clone();
                            let delete_this = delete_this.clone();
                            menu.item(PopupMenuItem::new("Rename").on_click(
                                move |_, window, cx| {
                                    if let Some(this) = rename_this.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.begin_collection_rename(
                                                rename_id.clone(),
                                                window,
                                                cx,
                                            );
                                        });
                                    }
                                },
                            ))
                            .item(
                                PopupMenuItem::new("Delete…").on_click(move |_, window, cx| {
                                    let delete_this = delete_this.clone();
                                    let delete_id = delete_id.clone();
                                    window.defer(cx, move |window, cx| {
                                        if let Some(this) = delete_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.open_collection_delete_dialog(
                                                    delete_id, window, cx,
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
