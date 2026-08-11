use super::*;

mod environment_variable_row;

pub(super) use environment_variable_row::EnvironmentVariableRow;

impl ApiTester {
    pub(super) fn render_environment_variable_grid(&self, cx: &mut Context<Self>) -> AnyElement {
        let can_update_definition = !self.sending && self.can_update_environment_definition();
        let can_update_values = !self.sending && self.can_update_environment_values_content();
        let can_delete_definition = !self.sending && self.can_delete_environment_content();
        let rows = self
            .environment_variables
            .iter()
            .enumerate()
            .map(|(index, row)| {
                self.render_environment_variable_row(
                    index,
                    row,
                    can_update_definition,
                    can_update_values,
                    can_delete_definition,
                    cx,
                )
            })
            .collect::<Vec<_>>();
        let empty = rows.is_empty();

        v_flex()
            .flex_1()
            .min_h_0()
            .rounded_lg()
            .border_1()
            .border_color(cx.api_outline_variant())
            .overflow_hidden()
            .bg(cx.api_surface())
            .child(
                h_flex()
                    .h(px(36.))
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
                            .w(px(112.))
                            .h_full()
                            .px_3()
                            .border_l_1()
                            .border_color(cx.api_outline_variant())
                            .flex()
                            .items_center()
                            .child("VISIBILITY"),
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
                    .id("environment-variable-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when(empty, |this| {
                        this.child(
                            v_flex()
                                .h(px(92.))
                                .items_center()
                                .justify_center()
                                .gap_1()
                                .text_color(cx.theme().muted_foreground)
                                .child(div().text_sm().child("No variables yet"))
                                .child(
                                    div()
                                        .text_xs()
                                        .child("Add one below to start templating requests."),
                                ),
                        )
                    })
                    .children(rows)
                    .child(
                        h_flex()
                            .h(px(44.))
                            .flex_shrink_0()
                            .px_3()
                            .border_t_1()
                            .border_color(cx.api_outline_variant())
                            .child(
                                Button::new("add-environment-variable")
                                    .icon(IconName::Plus)
                                    .label("Add variable")
                                    .small()
                                    .ghost()
                                    .disabled(!can_update_definition)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.push_environment_row(window, cx);
                                        if let Some(input) = this
                                            .environment_variables
                                            .last()
                                            .map(|row| row.key.clone())
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
