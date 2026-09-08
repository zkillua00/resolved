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
                let body_hover_input = self.body.read(cx).input_state();
                let body_click_input = body_hover_input.clone();
                let body_hover_this = cx.entity().downgrade();
                let body_hover_input_id = body_hover_input.entity_id();
                div()
                    .id("raw-body-template-source")
                    .size_full()
                    .on_hover(move |hovered, window, cx| {
                        if let Some(this) = body_hover_this.upgrade() {
                            this.update(cx, |this, cx| {
                                this.set_template_variable_source_hovered(
                                    body_hover_input_id,
                                    *hovered,
                                    window,
                                    cx,
                                );
                            });
                        }
                    })
                    .on_mouse_move(cx.listener(move |this, event, window, cx| {
                        this.hover_template_variable_popover(
                            body_hover_input.clone(),
                            event,
                            window,
                            cx,
                        );
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event, window, cx| {
                            this.open_template_variable_popover(
                                body_click_input.clone(),
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
            .child(
                div()
                    .flex_shrink_0()
                    .px_3()
                    .py_2()
                    .child(self.render_body_mode_toolbar(cx)),
            )
            .child(request_workspace::request_content_container(content))
            .into_any_element()
    }
}
