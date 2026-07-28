use super::*;

impl ApiTester {
    pub(super) fn render_environment_workspace(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .size_full()
            .min_w_0()
            .bg(cx.api_surface())
            .child(self.render_environment_browser(cx))
            .child(
                div()
                    .h_full()
                    .flex_1()
                    .min_w_0()
                    .child(self.render_environment_detail(cx)),
            )
            .into_any_element()
    }
}
