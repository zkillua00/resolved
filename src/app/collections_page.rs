use super::*;

mod collection_tree_row;
mod saved_request_tree_row;

impl ApiTester {
    pub(super) fn render_collections(&self, cx: &mut Context<Self>) -> AnyElement {
        let query = self
            .collection_search
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        let searching = !query.is_empty();
        let mut tree_rows = Vec::new();

        for (collection_index, collection) in self.workspace.collections.iter().enumerate() {
            let collection_matches = !searching || collection.name.to_lowercase().contains(&query);
            let matching_request_indexes = collection
                .requests
                .iter()
                .enumerate()
                .filter_map(|(index, request)| {
                    if !searching || collection_matches {
                        return Some(index);
                    }
                    let draft = &request.definition.request;
                    let matches = request.name.to_lowercase().contains(&query)
                        || draft.method.to_lowercase().contains(&query)
                        || draft.url.to_lowercase().contains(&query);
                    matches.then_some(index)
                })
                .collect::<Vec<_>>();

            if searching && !collection_matches && matching_request_indexes.is_empty() {
                continue;
            }

            let expanded = searching
                || self
                    .expanded_collection_ids
                    .contains(collection.id.as_str());
            tree_rows.push(self.render_collection_tree_row(collection_index, expanded, cx));

            if !expanded {
                continue;
            }

            tree_rows.extend(matching_request_indexes.into_iter().map(|request_index| {
                self.render_saved_request_tree_row(collection_index, request_index, cx)
            }));
        }

        v_flex()
            .size_full()
            .min_w_0()
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .bg(surface())
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
