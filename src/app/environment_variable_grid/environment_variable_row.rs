use super::*;

pub(in crate::app) struct EnvironmentVariableRow {
    pub(in crate::app) id: String,
    pub(in crate::app) key: Entity<InputState>,
    pub(in crate::app) value: Entity<InputState>,
    pub(in crate::app) enabled: bool,
    pub(in crate::app) secret: bool,
    pub(in crate::app) _subscriptions: Vec<Subscription>,
}

impl ApiTester {
    pub(super) fn render_environment_variable_row(
        &self,
        index: usize,
        row: &EnvironmentVariableRow,
        can_mutate: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let checkbox_id = row.id.clone();
        let secret_id = row.id.clone();
        let duplicate_id = row.id.clone();
        let remove_id = row.id.clone();
        let action_this = cx.entity().downgrade();
        let group_id: SharedString = format!("environment-variable-actions-{}", row.id).into();

        h_flex()
            .id(("environment-variable-grid-row", index))
            .group(group_id.clone())
            .w_full()
            .h(px(46.))
            .flex_shrink_0()
            .border_t_1()
            .border_color(outline_variant())
            .bg(surface())
            .hover(|style| style.bg(surface_low()))
            .when(!row.enabled, |this| this.opacity(0.58))
            .child(
                div()
                    .w(px(44.))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Checkbox::new(("environment-variable-enabled", index))
                            .checked(row.enabled)
                            .small()
                            .disabled(!can_mutate)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                if let Some(row) = this
                                    .environment_variables
                                    .iter_mut()
                                    .find(|row| row.id == checkbox_id)
                                {
                                    row.enabled = *checked;
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .border_l_1()
                    .border_color(outline_variant())
                    .child(
                        Input::new(&row.key)
                            .appearance(false)
                            .small()
                            .size_full()
                            .px_3()
                            .disabled(!can_mutate),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .border_l_1()
                    .border_color(outline_variant())
                    .child(
                        Input::new(&row.value)
                            .appearance(false)
                            .small()
                            .size_full()
                            .px_3()
                            .disabled(!can_mutate),
                    ),
            )
            .child(
                div()
                    .w(px(112.))
                    .h_full()
                    .flex_shrink_0()
                    .border_l_1()
                    .border_color(outline_variant())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Button::new(("secret-variable", index))
                            .icon(if row.secret {
                                IconName::EyeOff
                            } else {
                                IconName::Eye
                            })
                            .label(if row.secret { "Secret" } else { "Plain" })
                            .xsmall()
                            .ghost()
                            .selected(row.secret)
                            .disabled(!can_mutate)
                            .tooltip(if row.secret {
                                "Show as a plain variable"
                            } else {
                                "Mask this value in the UI and diagnostics"
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(row) = this
                                    .environment_variables
                                    .iter_mut()
                                    .find(|row| row.id == secret_id)
                                {
                                    row.secret = !row.secret;
                                    let masked = row.secret;
                                    row.value.update(cx, |input, cx| {
                                        input.set_masked(masked, window, cx);
                                    });
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .w(px(44.))
                    .h_full()
                    .flex_shrink_0()
                    .border_l_1()
                    .border_color(outline_variant())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Button::new(("environment-variable-actions", index))
                            .icon(IconName::EllipsisVertical)
                            .xsmall()
                            .ghost()
                            .rounded_full()
                            .invisible()
                            .group_hover(group_id, |style| style.visible())
                            .tooltip("Variable actions")
                            .dropdown_menu(move |menu, _, _| {
                                let duplicate_this = action_this.clone();
                                let duplicate_id = duplicate_id.clone();
                                let remove_this = action_this.clone();
                                let remove_id = remove_id.clone();
                                menu.item(
                                    PopupMenuItem::new("Duplicate")
                                        .disabled(!can_mutate)
                                        .on_click(move |_, window, cx| {
                                            if let Some(this) = duplicate_this.upgrade() {
                                                this.update(cx, |this, cx| {
                                                    this.duplicate_environment_row(
                                                        &duplicate_id,
                                                        window,
                                                        cx,
                                                    );
                                                });
                                            }
                                        }),
                                )
                                .separator()
                                .item(
                                    PopupMenuItem::new("Delete").disabled(!can_mutate).on_click(
                                        move |_, _, cx| {
                                            if let Some(this) = remove_this.upgrade() {
                                                this.update(cx, |this, cx| {
                                                    this.environment_variables
                                                        .retain(|row| row.id != remove_id);
                                                    cx.notify();
                                                });
                                            }
                                        },
                                    ),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }
}
