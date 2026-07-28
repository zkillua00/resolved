use super::super::*;

impl ApiTester {
    pub(in crate::app) fn render_body_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let content = match self.body_mode {
            BodyMode::None => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(cx.theme().muted_foreground)
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child("This request has no body"),
                )
                .child(
                    div()
                        .text_xs()
                        .child("Choose Raw or a form mode above to add one."),
                )
                .into_any_element(),
            BodyMode::Raw => {
                let body_template_input = self.body.read(cx).input_state();
                div()
                    .size_full()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event, window, cx| {
                            this.open_template_variable_popover(
                                body_template_input.clone(),
                                event,
                                window,
                                cx,
                            );
                        }),
                    )
                    .child(self.body.clone())
                    .into_any_element()
            }
            BodyMode::FormUrlEncoded => self.render_body_fields_editor(false, cx),
            BodyMode::MultipartFormData => self.render_body_fields_editor(true, cx),
        };

        v_flex()
            .size_full()
            .min_h_0()
            .gap_3()
            .child(self.render_body_mode_toolbar(cx))
            .child(div().flex_1().min_h_0().child(content))
            .into_any_element()
    }
}
