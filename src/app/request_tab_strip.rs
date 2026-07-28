use super::*;

impl ApiTester {
    pub(super) fn render_request_tab_strip(&self, cx: &mut Context<Self>) -> AnyElement {
        let active_id = self.request_tabs.active_tab_id().clone();
        let tabs = self
            .request_tabs
            .tabs()
            .iter()
            .map(|tab| {
                let tab_id = tab.id().clone();
                let activate_id = tab_id.clone();
                let close_id = tab_id.clone();
                let tab_key = tab_id.as_str();
                let row_id: SharedString = format!("open-request-tab-{tab_key}").into();
                let row_group: SharedString = format!("open-request-tab-group-{tab_key}").into();
                let title_id: SharedString = format!("open-request-tab-title-{tab_key}").into();
                let close_button_id: SharedString =
                    format!("close-open-request-tab-{tab_key}").into();
                let active = tab_id == active_id;
                let dirty = if active {
                    self.request_is_dirty()
                } else {
                    tab.is_dirty()
                };
                let title = compact_label(tab.display_title(), 28);
                let tooltip = tab.display_title().to_owned();

                h_flex()
                    .id(row_id)
                    .group(row_group)
                    .h_full()
                    .flex_shrink_0()
                    .min_w(px(148.))
                    .max_w(px(240.))
                    .px_3()
                    .gap_2()
                    .border_r_1()
                    .border_color(outline_variant())
                    .cursor_pointer()
                    .when(active, |this| {
                        this.bg(surface())
                            .border_b_2()
                            .border_color(cx.theme().primary)
                    })
                    .when(!active, |this| {
                        this.bg(surface_low())
                            .hover(|style| style.bg(surface_container()))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.activate_request_tab(activate_id.clone(), window, cx);
                    }))
                    .child(
                        div()
                            .id(title_id)
                            .min_w_0()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_sm()
                            .font_medium()
                            .tooltip(move |window, cx| {
                                Tooltip::new(tooltip.clone()).build(window, cx)
                            })
                            .child(title),
                    )
                    .when(dirty, |this| {
                        this.child(
                            div()
                                .size(px(7.))
                                .flex_shrink_0()
                                .rounded_full()
                                .bg(cx.theme().warning),
                        )
                    })
                    .child(
                        Button::new(close_button_id)
                            .icon(IconName::Close)
                            .xsmall()
                            .ghost()
                            .rounded_full()
                            .tooltip("Close request tab")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.request_close_request_tab(close_id.clone(), window, cx);
                            })),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        h_flex()
            .h(px(42.))
            .w_full()
            .flex_shrink_0()
            .border_b_1()
            .border_color(outline_variant())
            .bg(surface_low())
            .child(
                h_flex()
                    .id("request-tabs-scroll")
                    .h_full()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_scroll()
                    .children(tabs),
            )
            .when_some(self.request_tabs_warning.clone(), |this, warning| {
                this.child(
                    Button::new("request-tabs-warning")
                        .icon(IconName::Info)
                        .xsmall()
                        .ghost()
                        .text_color(cx.theme().warning)
                        .tooltip(warning),
                )
            })
            .child(
                div()
                    .h_full()
                    .px_2()
                    .flex()
                    .items_center()
                    .border_l_1()
                    .border_color(outline_variant())
                    .child(
                        Button::new("new-request-tab")
                            .icon(IconName::Plus)
                            .small()
                            .ghost()
                            .rounded_full()
                            .tooltip("New request tab")
                            .disabled(self.sending)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_blank_request_tab(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}
