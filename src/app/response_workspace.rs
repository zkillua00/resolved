use super::*;

impl ApiTester {
    pub(super) fn render_response_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(response) = &self.response else {
            let has_script_console = self.pre_script_report.is_some()
                || self.post_script_report.is_some()
                || self.script_diagnostic.is_some();
            let state_label = if self.sending {
                "Waiting for response…"
            } else if self.script_diagnostic.is_some() {
                "Script failed"
            } else if self.request_error.is_some() {
                "Request failed"
            } else {
                "No response yet"
            };
            return v_flex()
                .size_full()
                .min_h_0()
                .bg(surface())
                .child(
                    h_flex()
                        .h(px(56.))
                        .flex_shrink_0()
                        .px_4()
                        .gap_3()
                        .border_b_1()
                        .border_color(outline_variant())
                        .child(div().text_base().font_semibold().child("Response"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(state_label),
                        ),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_h_0()
                        .when(has_script_console, |this| {
                            this.p_4().child(self.render_script_results(cx))
                        })
                        .when(!has_script_console, |this| {
                            this.items_center()
                                .justify_center()
                                .gap_2()
                                .p_4()
                                .text_color(cx.theme().muted_foreground)
                                .when_some(self.request_error.clone(), |this, error| {
                                    this.child(
                                        div()
                                            .max_w(px(640.))
                                            .px_4()
                                            .py_3()
                                            .rounded_lg()
                                            .border_1()
                                            .border_color(cx.theme().danger)
                                            .bg(cx.theme().danger.opacity(0.08))
                                            .text_color(cx.theme().danger)
                                            .text_sm()
                                            .child(error),
                                    )
                                })
                                .when(self.request_error.is_none() && !self.sending, |this| {
                                    this.child(
                                        div()
                                            .text_base()
                                            .font_semibold()
                                            .text_color(cx.theme().foreground)
                                            .child("Ready to send"),
                                    )
                                    .child(div().text_sm().child(
                                        "Choose a method, enter a URL, then press Send or Return.",
                                    ))
                                })
                                .when(self.sending, |this| {
                                    this.child(
                                        div()
                                            .text_base()
                                            .font_semibold()
                                            .text_color(cx.theme().foreground)
                                            .child("Waiting for response…"),
                                    )
                                })
                        }),
                )
                .into_any_element();
        };

        v_flex()
            .size_full()
            .min_h_0()
            .bg(surface())
            .child(
                h_flex()
                    .h(px(56.))
                    .flex_shrink_0()
                    .px_4()
                    .gap_4()
                    .justify_between()
                    .border_b_1()
                    .border_color(outline_variant())
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_4()
                            .child(div().text_base().font_semibold().child("Response"))
                            .child(self.render_response_summary(response, cx)),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .when(self.response_tab == ResponseTab::Body, |this| {
                                this.child(
                                    Button::new("toggle-pretty")
                                        .label(if self.pretty_body { "Pretty" } else { "Raw" })
                                        .small()
                                        .ghost()
                                        .rounded(px(18.))
                                        .selected(self.pretty_body)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.pretty_body = !this.pretty_body;
                                            this.copied = false;
                                            if let Some(response) = this.response.clone() {
                                                this.update_response_editor(&response, window, cx);
                                            }
                                            cx.notify();
                                        })),
                                )
                            })
                            .when(self.response_tab != ResponseTab::Scripts, |this| {
                                this.child(
                                    Button::new("copy-response")
                                        .label(if self.copied { "Copied" } else { "Copy" })
                                        .small()
                                        .ghost()
                                        .rounded(px(18.))
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.copy_response(cx)),
                                        ),
                                )
                            }),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .h(px(42.))
                    .flex_shrink_0()
                    .px_4()
                    .justify_between()
                    .border_b_1()
                    .border_color(outline_variant())
                    .child(
                        TabBar::new("response-tabs")
                            .underline()
                            .children(["Body", "Headers", "Preview", "Scripts"])
                            .selected_index(self.response_tab.index())
                            .on_click(cx.listener(|this, index: &usize, window, cx| {
                                this.select_response_tab(*index, window, cx);
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .ml_3()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_right()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(compact_url(&response.final_url)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .when(self.response_tab == ResponseTab::Body, |this| {
                        this.child(self.render_response_body(cx))
                    })
                    .when(self.response_tab == ResponseTab::Headers, |this| {
                        this.child(self.render_response_headers(response, cx))
                    })
                    .when(self.response_tab == ResponseTab::Preview, |this| {
                        this.child(self.render_preview(response, cx))
                    })
                    .when(self.response_tab == ResponseTab::Scripts, |this| {
                        this.child(self.render_script_results(cx))
                    }),
            )
            .into_any_element()
    }
}
