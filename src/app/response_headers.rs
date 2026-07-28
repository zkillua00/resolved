use super::*;

impl ApiTester {
    pub(super) fn render_response_headers(
        &self,
        response: &ResponseData,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id("response-headers-scroll")
            .size_full()
            .overflow_y_scroll()
            .children(response.headers.iter().enumerate().map(|(index, header)| {
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_4()
                    .when(index > 0, |this| {
                        this.border_t_1().border_color(cx.theme().border)
                    })
                    .child(
                        div()
                            .w(px(220.))
                            .flex_shrink_0()
                            .text_sm()
                            .font_semibold()
                            .child(header.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_family(cx.theme().mono_font_family.clone())
                            .child(header.value.clone()),
                    )
            }))
            .into_any_element()
    }
}
