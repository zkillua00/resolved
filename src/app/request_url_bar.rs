use super::*;

impl ApiTester {
    pub(super) fn render_url_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let method = self.method.read(cx).value().trim().to_ascii_uppercase();
        let color = method_color(&method, cx);
        let selected_method = method.clone();
        let this = cx.entity().downgrade();
        let url_hover_input = self.url.clone();
        let url_click_input = self.url.clone();
        let url_hover_this = cx.entity().downgrade();
        let url_hover_input_id = self.url.entity_id();
        let action = if self.sending {
            Button::new("cancel-request")
                .label("Cancel")
                .large()
                .h(px(44.))
                .rounded(px(12.))
                .danger()
                .on_click(cx.listener(|this, _, _, cx| this.cancel_request(cx)))
        } else {
            Button::new("send-request")
                .label("Send")
                .large()
                .h(px(44.))
                .rounded(px(12.))
                .primary()
                .on_click(
                    cx.listener(|this, _: &ClickEvent, window, cx| this.start_request(window, cx)),
                )
        };

        h_flex()
            .w_full()
            .h(px(44.))
            .gap_3()
            .child(
                h_flex()
                    .h_full()
                    .flex_1()
                    .min_w_0()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.api_outline_variant())
                    .bg(cx.api_surface_container())
                    .overflow_hidden()
                    .child(
                        h_flex()
                            .h_full()
                            .w(px(148.))
                            .flex_shrink_0()
                            .border_r_1()
                            .border_color(cx.api_outline_variant())
                            .bg(color.opacity(0.08))
                            .child(
                                Popover::new("method-options")
                                    .appearance(false)
                                    .anchor(gpui::Corner::TopLeft)
                                    .track_focus(&self.method.read(cx).focus_handle(cx))
                                    .trigger(
                                        Input::new(&self.method)
                                            .appearance(false)
                                            .large()
                                            .w(px(148.))
                                            .font_semibold()
                                            .text_color(color)
                                            .suffix(
                                                gpui_component::Icon::new(IconName::ChevronDown)
                                                    .small()
                                                    .text_color(color),
                                            ),
                                    )
                                    .content(move |_, _, cx| {
                                        let popover = cx.entity();
                                        v_flex()
                                            .w(px(200.))
                                            .py_1()
                                            .rounded_lg()
                                            .border_1()
                                            .border_color(cx.api_outline_variant())
                                            .bg(cx.api_surface_container())
                                            .overflow_hidden()
                                            .children(STANDARD_HTTP_METHODS.iter().enumerate().map(
                                                |(index, method)| {
                                                    let method = (*method).to_owned();
                                                    let click_method = method.clone();
                                                    let this = this.clone();
                                                    let popover = popover.clone();
                                                    let selected = selected_method == method;
                                                    h_flex()
                                                        .id(("method-option", index))
                                                        .h(px(36.))
                                                        .w_full()
                                                        .px_3()
                                                        .gap_2()
                                                        .cursor_pointer()
                                                        .when(selected, |this| {
                                                            this.bg(cx.theme().sidebar_accent)
                                                        })
                                                        .hover(|style| style.bg(cx.theme().accent))
                                                        .child(
                                                            div().w(px(16.)).flex_shrink_0().when(
                                                                selected,
                                                                |this| {
                                                                    this.child(
                                                                        gpui_component::Icon::new(
                                                                            IconName::Check,
                                                                        )
                                                                        .xsmall(),
                                                                    )
                                                                },
                                                            ),
                                                        )
                                                        .child(
                                                            div()
                                                                .flex_1()
                                                                .font_semibold()
                                                                .text_color(method_color(
                                                                    &method, cx,
                                                                ))
                                                                .child(method),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(
                                                                    cx.theme().muted_foreground,
                                                                )
                                                                .child("HTTP"),
                                                        )
                                                        .on_click(move |_, window, cx| {
                                                            if let Some(this) = this.upgrade() {
                                                                this.update(cx, |this, cx| {
                                                                    this.method.update(
                                                                        cx,
                                                                        |state, cx| {
                                                                            state.set_value(
                                                                                click_method
                                                                                    .clone(),
                                                                                window,
                                                                                cx,
                                                                            );
                                                                            state.focus(window, cx);
                                                                        },
                                                                    );
                                                                    cx.notify();
                                                                });
                                                            }
                                                            popover.update(cx, |popover, cx| {
                                                                popover.dismiss(window, cx);
                                                            });
                                                        })
                                                },
                                            ))
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("request-url-template-source")
                            .flex_1()
                            .min_w_0()
                            .on_hover(move |hovered, window, cx| {
                                if let Some(this) = url_hover_this.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.set_template_variable_source_hovered(
                                            url_hover_input_id,
                                            *hovered,
                                            window,
                                            cx,
                                        );
                                    });
                                }
                            })
                            .on_mouse_move(cx.listener(move |this, event, window, cx| {
                                this.hover_template_variable_popover(
                                    url_hover_input.clone(),
                                    event,
                                    window,
                                    cx,
                                );
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event, window, cx| {
                                    this.open_template_variable_popover(
                                        url_click_input.clone(),
                                        event,
                                        window,
                                        cx,
                                    );
                                }),
                            )
                            .child(Input::new(&self.url).appearance(false).large()),
                    ),
            )
            .child(action)
            .into_any_element()
    }
}
