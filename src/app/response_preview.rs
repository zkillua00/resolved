use super::*;

impl ApiTester {
    pub(super) fn render_preview(
        &self,
        response: &ResponseData,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !can_preview(response.content_type.as_deref(), &response.body) {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(cx.theme().muted_foreground)
                .child(div().text_sm().font_semibold().child("No HTML preview"))
                .child(div().text_xs().child(
                    "The response is not declared as HTML and has no HTML document markers.",
                ))
                .into_any_element();
        }

        v_flex()
            .size_full()
            .min_h_0()
            .child(
                h_flex()
                    .h_8()
                    .flex_shrink_0()
                    .px_3()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().muted.opacity(0.45))
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Captured response · JavaScript, network access, navigation, and downloads blocked"),
            )
            .when_some(self.preview_error.clone(), |this, error| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    // WebKit is a native child window and remains hidden until
                    // its document has painted. Keep the host pane opaque in
                    // the meantime instead of exposing the desktop behind it.
                    .bg(gpui::white())
                    .border_1()
                    .border_t_0()
                    .border_color(cx.theme().border)
                    .when_some(self.preview.clone(), |this, preview| this.child(preview)),
            )
            .into_any_element()
    }
}
