use super::*;

impl ApiTester {
    pub(super) fn render_environment_browser(&self, cx: &mut Context<Self>) -> AnyElement {
        let can_mutate = !self.sending && self.workspace_writable;
        let query = self
            .environment_search
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        let searching = !query.is_empty();
        let this = cx.entity().downgrade();
        let rows = self
            .workspace
            .environments
            .iter()
            .enumerate()
            .filter(|(_, environment)| {
                !searching || environment.name.to_lowercase().contains(&query)
            })
            .map(|(index, environment)| {
                let select_id = environment.id.clone();
                let action_environment_id = environment.id.clone();
                let action_environment_name = environment.name.clone();
                let selected = self.selected_environment_id.as_deref() == Some(&environment.id);
                let active =
                    self.workspace.active_environment_id.as_deref() == Some(&environment.id);
                let variable_count = environment.variables.len();
                let group_id: SharedString =
                    format!("environment-actions-{}", environment.id).into();
                let action_this = this.clone();

                h_flex()
                    .id(("environment-browser-row", index))
                    .group(group_id.clone())
                    .w_full()
                    .h(px(52.))
                    .px_2()
                    .gap_2()
                    .rounded_md()
                    .when(selected, |this| this.bg(cx.theme().sidebar_accent))
                    .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.72)))
                    .child(
                        div()
                            .size_2()
                            .flex_shrink_0()
                            .rounded_full()
                            .border_1()
                            .border_color(if active {
                                cx.theme().primary
                            } else {
                                cx.theme().muted_foreground.opacity(0.5)
                            })
                            .when(active, |this| this.bg(cx.theme().primary)),
                    )
                    .child(
                        v_flex()
                            .id(("select-environment-browser-row", index))
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .justify_center()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_environment(select_id.clone(), window, cx);
                            }))
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_sm()
                                    .font_semibold()
                                    .child(environment.name.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if active {
                                        format!("Active · {variable_count} variables")
                                    } else {
                                        format!("{variable_count} variables")
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .w(px(28.))
                            .h_full()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Button::new(("environment-browser-actions", index))
                                    .icon(IconName::EllipsisVertical)
                                    .xsmall()
                                    .ghost()
                                    .rounded_full()
                                    .tooltip("Environment actions")
                                    .invisible()
                                    .group_hover(group_id, |style| style.visible())
                                    .dropdown_menu(move |menu, _, _| {
                                        let activation_this = action_this.clone();
                                        let activation_id = action_environment_id.clone();
                                        let delete_this = action_this.clone();
                                        let delete_id = action_environment_id.clone();
                                        menu.item(
                                            PopupMenuItem::new(if active {
                                                "Stop using for requests"
                                            } else {
                                                "Use for requests"
                                            })
                                            .checked(active)
                                            .disabled(!can_mutate)
                                            .on_click(move |_, _, cx| {
                                                if let Some(this) = activation_this.upgrade() {
                                                    this.update(cx, |this, cx| {
                                                        this.activate_environment(
                                                            (!active)
                                                                .then(|| activation_id.clone()),
                                                            cx,
                                                        );
                                                    });
                                                }
                                            }),
                                        )
                                        .separator()
                                        .item(
                                            PopupMenuItem::new(format!(
                                                "Delete “{}”…",
                                                compact_label(&action_environment_name, 22)
                                            ))
                                            .disabled(!can_mutate)
                                            .on_click(move |_, window, cx| {
                                                let delete_this = delete_this.clone();
                                                let delete_id = delete_id.clone();
                                                window.defer(cx, move |window, cx| {
                                                    if let Some(this) = delete_this.upgrade() {
                                                        this.update(cx, |this, cx| {
                                                            this.open_environment_delete_dialog(
                                                                delete_id, window, cx,
                                                            );
                                                        });
                                                    }
                                                });
                                            }),
                                        )
                                    }),
                            ),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let no_matches = searching && rows.is_empty();
        let no_environment_active = self.workspace.active_environment_id.is_none();

        v_flex()
            .w(px(288.))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .bg(cx.api_surface_low())
            .child(
                h_flex()
                    .h(px(56.))
                    .px_4()
                    .flex_shrink_0()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.theme().sidebar_border)
                    .child(div().text_base().font_semibold().child("Environments"))
                    .child(
                        Button::new("create-environment")
                            .icon(IconName::Plus)
                            .small()
                            .ghost()
                            .rounded_full()
                            .tooltip("New environment")
                            .disabled(!can_mutate)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.create_environment(window, cx);
                            })),
                    ),
            )
            .child(
                h_flex()
                    .h(px(56.))
                    .px_3()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(cx.theme().sidebar_border)
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.environment_search)
                                .prefix(IconName::Search)
                                .cleanable(true),
                        ),
                    ),
            )
            .child(
                v_flex()
                    .id("environments-browser-scroll")
                    .flex_1()
                    .min_h_0()
                    .gap_1()
                    .p_2()
                    .overflow_y_scroll()
                    .child(
                        h_flex()
                            .id("no-environment-row")
                            .w_full()
                            .h(px(52.))
                            .px_3()
                            .gap_3()
                            .rounded_md()
                            .cursor_pointer()
                            .when(no_environment_active, |this| {
                                this.bg(cx.theme().sidebar_accent)
                            })
                            .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.72)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.activate_environment(None, cx);
                            }))
                            .child(
                                div()
                                    .size_2()
                                    .flex_shrink_0()
                                    .rounded_full()
                                    .border_1()
                                    .border_color(if no_environment_active {
                                        cx.theme().primary
                                    } else {
                                        cx.theme().muted_foreground.opacity(0.5)
                                    })
                                    .when(no_environment_active, |this| {
                                        this.bg(cx.theme().primary)
                                    }),
                            )
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .child(div().text_sm().font_semibold().child("No environment"))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Send requests without variables"),
                                    ),
                            ),
                    )
                    .children(rows)
                    .when(no_matches, |this| {
                        this.child(
                            v_flex()
                                .items_center()
                                .gap_1()
                                .px_4()
                                .py_6()
                                .text_center()
                                .text_color(cx.theme().muted_foreground)
                                .child(div().text_sm().child("No matching environments"))
                                .child(div().text_xs().child("Try another environment name.")),
                        )
                    }),
            )
            .when_some(self.workspace_warning.clone(), |this, warning| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(cx.theme().warning)
                        .text_xs()
                        .text_color(cx.theme().warning)
                        .child(warning),
                )
            })
            .into_any_element()
    }
}
