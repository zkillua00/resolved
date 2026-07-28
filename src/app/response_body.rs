use super::*;

impl ApiTester {
    pub(super) fn render_response_summary(
        &self,
        response: &ResponseData,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let status_color = status_color(response.status, cx);
        h_flex()
            .gap_3()
            .text_xs()
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(status_color.opacity(0.14))
                    .text_color(status_color)
                    .font_semibold()
                    .child(format!("{} {}", response.status, response.status_text)),
            )
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(format_duration(response.duration)),
            )
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(format_bytes(response.size_bytes())),
            )
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(response.http_version.clone()),
            )
            .into_any_element()
    }

    pub(super) fn render_response_body(&self, _cx: &mut Context<Self>) -> AnyElement {
        div()
            .size_full()
            .bg(surface_lowest())
            .child(self.response_editor.clone())
            .into_any_element()
    }
}
