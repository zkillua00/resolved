use super::*;

mod header_row;

pub(super) use header_row::HeaderRow;

impl ApiTester {
    pub(super) fn render_headers_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows = self
            .headers
            .iter()
            .map(|row| self.render_header_row(row, cx))
            .collect::<Vec<_>>();
        let enabled_count = self.request_header_count(cx);

        v_flex()
            .size_full()
            .min_h_0()
            .rounded_lg()
            .border_1()
            .border_color(cx.api_outline_variant())
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
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .child(format!("{enabled_count} enabled")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Sent in row order"),
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
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .px_3()
                            .border_l_1()
                            .border_color(cx.api_outline_variant())
                            .flex()
                            .items_center()
                            .child("KEY"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .px_3()
                            .border_l_1()
                            .border_color(cx.api_outline_variant())
                            .flex()
                            .items_center()
                            .child("VALUE"),
                    )
                    .child(
                        div()
                            .w(px(64.))
                            .h_full()
                            .border_l_1()
                            .border_color(cx.api_outline_variant())
                            .flex()
                            .items_center()
                            .justify_center()
                            .child("SHARE"),
                    )
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
                    .id("header-rows")
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
                                Button::new("add-header-row")
                                    .icon(IconName::Plus)
                                    .label("Add header")
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.push_header_row("", "", true, true, window, cx);
                                        if let Some(input) =
                                            this.headers.last().map(|row| row.name.clone())
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
}
