use super::*;

pub(in crate::app) struct QueryParamRow {
    pub(in crate::app) id: usize,
    pub(in crate::app) key: Entity<InputState>,
    pub(in crate::app) value: Entity<InputState>,
    pub(in crate::app) enabled: bool,
    pub(in crate::app) _subscriptions: Vec<Subscription>,
}

impl ApiTester {
    pub(super) fn render_query_params_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows = self
            .query_params
            .iter()
            .map(|row| self.render_query_param_row(row, cx))
            .collect::<Vec<_>>();
        let enabled_count = self.request_query_param_count(cx);

        v_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .bg(cx.api_surface())
            .child(
                h_flex()
                    .h(px(42.))
                    .w_full()
                    .flex_shrink_0()
                    .px_3()
                    .justify_between()
                    .bg(cx.api_surface_low())
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().text_sm().font_semibold().child("Query Params"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{enabled_count} enabled")),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .h(px(34.))
                    .w_full()
                    .flex_shrink_0()
                    .bg(cx.api_surface_low())
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(div().w(px(44.)).child(""))
                    .child(query_param_heading("KEY", cx))
                    .child(query_param_heading("VALUE", cx))
                    .child(query_param_heading("DESCRIPTION", cx))
                    .child(
                        div()
                            .w(px(44.))
                            .h_full()
                            .border_l_1()
                            .border_color(cx.api_outline_variant()),
                    ),
            )
            .child(
                v_flex()
                    .id("query-param-rows")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows)
                    .child(
                        h_flex()
                            .h(px(42.))
                            .flex_shrink_0()
                            .px_3()
                            .border_t_1()
                            .border_color(cx.api_outline_variant())
                            .child(
                                Button::new("add-query-param-row")
                                    .icon(IconName::Plus)
                                    .label("Add parameter")
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.push_query_param_row("", "", true, window, cx);
                                        if let Some(input) =
                                            this.query_params.last().map(|row| row.key.clone())
                                        {
                                            input.read(cx).focus_handle(cx).focus(window);
                                        }
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_query_param_row(&self, row: &QueryParamRow, cx: &mut Context<Self>) -> AnyElement {
        let id = row.id;
        let description = documentation::explanation(
            &self.documentation_intelligence,
            &self.documentation,
            crate::documentation_intelligence::TargetKind::Query,
            row.key.read(cx).value().as_ref(),
            cx,
        );
        h_flex()
            .id(("query-param-row", id))
            .w_full()
            .h(px(44.))
            .flex_shrink_0()
            .border_t_1()
            .border_color(cx.api_outline_variant())
            .bg(cx.api_surface())
            .hover(|style| style.bg(cx.api_surface_low()))
            .when(!row.enabled, |this| this.opacity(0.55))
            .child(
                div()
                    .w(px(44.))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Checkbox::new(("query-param-enabled", id))
                            .checked(row.enabled)
                            .small()
                            .on_click(cx.listener(move |this, checked: &bool, window, cx| {
                                this.toggle_query_param_row(id, *checked, window, cx);
                            })),
                    ),
            )
            .child(
                query_param_input_cell(&row.key, cx)
                    .id(("query-param-key-description", id))
                    .when_some(description.clone(), |this, description| {
                        this.tooltip(move |window, cx| Tooltip::new(description.clone()).build(window, cx))
                    }),
            )
            .child(query_param_input_cell(&row.value, cx))
            .child(documentation::description_cell(
                format!("query-description-{id}").into(),
                description,
                cx,
            ))
            .child(
                div()
                    .w(px(44.))
                    .h_full()
                    .flex_shrink_0()
                    .border_l_1()
                    .border_color(cx.api_outline_variant())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Button::new(("remove-query-param", id))
                            .icon(IconName::Delete)
                            .xsmall()
                            .ghost()
                            .tooltip("Delete parameter")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remove_query_param_row(id, window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}

fn query_param_heading(label: &'static str, cx: &App) -> impl IntoElement {
    div()
        .flex_1()
        .min_w_0()
        .h_full()
        .px_3()
        .border_l_1()
        .border_color(cx.api_outline_variant())
        .flex()
        .items_center()
        .child(label)
}

fn query_param_input_cell(input: &Entity<InputState>, cx: &App) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .h_full()
        .border_l_1()
        .border_color(cx.api_outline_variant())
        .child(
            Input::new(input)
                .appearance(false)
                .small()
                .size_full()
                .px_3(),
        )
}
