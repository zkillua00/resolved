use super::super::*;

impl ApiTester {
    pub(super) fn render_body_fields_editor(
        &self,
        multipart: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this = cx.entity().downgrade();
        let rows = self
            .body_fields
            .iter()
            .map(|row| self.render_body_field_row(row, multipart, this.clone(), cx))
            .collect::<Vec<_>>();
        let enabled_count = self
            .body_fields
            .iter()
            .filter(|row| row.enabled && !input_text_is_blank(&row.name, cx))
            .count();

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
                                    .child(if multipart {
                                        "Text fields and local file uploads"
                                    } else {
                                        "Encoded and sent in row order"
                                    }),
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
                    .when(multipart, |this| {
                        this.child(
                            div()
                                .w(px(96.))
                                .h_full()
                                .px_3()
                                .border_l_1()
                                .border_color(cx.api_outline_variant())
                                .flex()
                                .items_center()
                                .child("TYPE"),
                        )
                    })
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
                            .w(px(44.))
                            .h_full()
                            .border_l_1()
                            .border_color(cx.api_outline_variant()),
                    ),
            )
            .child(
                v_flex()
                    .id("body-field-rows")
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
                                Button::new("add-body-field")
                                    .icon(IconName::Plus)
                                    .label("Add field")
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.push_body_field_row(
                                            "",
                                            "",
                                            true,
                                            BodyFieldKind::Text,
                                            window,
                                            cx,
                                        );
                                        if let Some(input) =
                                            this.body_fields.last().map(|row| row.name.clone())
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
