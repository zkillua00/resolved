use super::*;

impl ApiTester {
    pub(super) fn render_template_variable_popover(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let popover = self.template_variable_popover.clone()?;
        let blocker = self.template_variable_mutation_blocker(&popover, cx);
        let has_environment = popover.expected_environment_id.is_some();
        let is_create = matches!(popover.action, TemplateVariableAction::Create);
        let title = if is_create {
            "Create environment variable"
        } else {
            "Enable environment variable"
        };
        let action_label = if is_create { "Create" } else { "Enable" };
        let template = format!("{{{{{}}}}}", popover.name);
        let environment = popover
            .environment_name
            .clone()
            .unwrap_or_else(|| "No active environment".to_owned());
        let apply_this = cx.entity().downgrade();
        let close_this = apply_this.clone();
        let outside_this = apply_this.clone();

        let content = v_flex()
            .id("template-variable-popover")
            .w(px(340.))
            .gap_3()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(cx.api_outline_variant())
            .bg(cx.api_surface_container())
            .shadow_lg()
            .on_mouse_down_out(move |_, _, cx| {
                if let Some(this) = outside_this.upgrade() {
                    this.update(cx, |this, cx| this.close_template_variable_popover(cx));
                }
            })
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().font_semibold().child(title))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{template} · {environment}")),
                    ),
            )
            .when(!has_environment, |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().warning)
                        .child("Choose an active environment before creating this variable."),
                )
                .child(
                    Button::new("template-open-environments")
                        .label("Open environments")
                        .small()
                        .outline()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_environments_from_template(window, cx);
                        })),
                )
            })
            .when(has_environment && is_create, |this| {
                this.child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child("VALUE"),
                        )
                        .child(
                            div()
                                .h(px(38.))
                                .w_full()
                                .rounded_md()
                                .border_1()
                                .border_color(cx.api_outline_variant())
                                .bg(cx.api_surface_lowest())
                                .child(
                                    Input::new(&popover.value)
                                        .appearance(false)
                                        .small()
                                        .size_full()
                                        .px_3()
                                        .disabled(blocker.is_some()),
                                ),
                        ),
                )
            })
            .when(has_environment && !is_create, |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "This variable exists but is disabled. Enable it without changing its value or secret status.",
                        ),
                )
            })
            .when_some(popover.error.clone().or_else(|| blocker.clone()), |this, message| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().red)
                        .child(message),
                )
            })
            .when(has_environment, |this| {
                this.child(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("template-variable-cancel")
                                .label("Cancel")
                                .small()
                                .ghost()
                                .on_click(move |_, _, cx| {
                                    if let Some(this) = close_this.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.close_template_variable_popover(cx)
                                        });
                                    }
                                }),
                        )
                        .child(
                            Button::new("template-variable-apply")
                                .label(action_label)
                                .small()
                                .primary()
                                .disabled(blocker.is_some())
                                .on_click(move |_, window, cx| {
                                    if let Some(this) = apply_this.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.apply_template_variable_action(window, cx)
                                        });
                                    }
                                }),
                        ),
                )
            });

        Some(
            deferred(
                anchored()
                    .position(popover.position)
                    .anchor(Corner::TopLeft)
                    .snap_to_window_with_margin(px(12.))
                    .child(content),
            )
            .with_priority(4)
            .into_any_element(),
        )
    }
}
