use super::*;

impl ApiTester {
    pub(super) fn render_environment_detail(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(environment) = self
            .selected_environment_id
            .as_deref()
            .and_then(|id| self.workspace.environment(id))
        else {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .p_8()
                .text_center()
                .bg(cx.api_surface())
                .child(
                    gpui_component::Icon::new(IconName::Globe)
                        .with_size(px(28.))
                        .text_color(cx.theme().muted_foreground),
                )
                .child(
                    div()
                        .text_lg()
                        .font_semibold()
                        .child("Create an environment"),
                )
                .child(
                    div()
                        .max_w(px(460.))
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "Keep reusable base URLs, tokens, and request values together, then reference them with {{variable}}.",
                        ),
                )
                .child(
                    Button::new("create-first-environment")
                        .icon(IconName::Plus)
                        .label("New environment")
                        .primary()
                        .disabled(self.sending || !self.workspace_writable)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.create_environment(window, cx);
                        })),
                )
                .into_any_element();
        };

        let environment_id = environment.id.clone();
        let environment_name = environment.name.clone();
        let editor_dirty = self.environment_editor_is_dirty(cx);
        let selected_is_active =
            self.workspace.active_environment_id.as_deref() == Some(environment_id.as_str());
        let active_editor_dirty = selected_is_active && editor_dirty;
        let variable_count = self.environment_variables.len();
        let can_mutate = !self.sending && self.workspace_writable;
        let activate_id = environment_id.clone();
        let delete_id = environment_id.clone();
        let delete_this = cx.entity().downgrade();

        v_flex()
            .size_full()
            .min_w_0()
            .bg(cx.api_surface())
            .child(
                h_flex()
                    .h(px(76.))
                    .flex_shrink_0()
                    .px_6()
                    .gap_4()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.api_outline_variant())
                    .child(
                        v_flex()
                            .w(px(440.))
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("ENVIRONMENT NAME"),
                            )
                            .child(
                                Input::new(&self.environment_name)
                                    .large()
                                    .disabled(!can_mutate),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .gap_2()
                            .when(editor_dirty, |this| {
                                this.child(
                                    div()
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .bg(cx.theme().warning.opacity(0.12))
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(cx.theme().warning)
                                        .child(if active_editor_dirty {
                                            "Unsaved · blocks Send"
                                        } else {
                                            "Unsaved"
                                        }),
                                )
                            })
                            .child(
                                Button::new("activate-selected-environment")
                                    .label(if selected_is_active {
                                        "Active"
                                    } else {
                                        "Use for requests"
                                    })
                                    .small()
                                    .outline()
                                    .selected(selected_is_active)
                                    .disabled(!can_mutate)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.activate_environment(
                                            if selected_is_active {
                                                None
                                            } else {
                                                Some(activate_id.clone())
                                            },
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                Button::new("revert-environment")
                                    .label("Revert")
                                    .small()
                                    .ghost()
                                    .disabled(!can_mutate || !editor_dirty)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.revert_environment(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("save-environment")
                                    .label("Save")
                                    .small()
                                    .primary()
                                    .disabled(!can_mutate || !editor_dirty)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save_environment(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("selected-environment-actions")
                                    .icon(IconName::EllipsisVertical)
                                    .small()
                                    .ghost()
                                    .rounded_full()
                                    .tooltip("Environment actions")
                                    .dropdown_menu(move |menu, _, _| {
                                        let delete_this = delete_this.clone();
                                        let delete_id = delete_id.clone();
                                        menu.item(
                                            PopupMenuItem::new(format!(
                                                "Delete “{}”…",
                                                compact_label(&environment_name, 24)
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
                    ),
            )
            .child(
                h_flex()
                    .h(px(42.))
                    .flex_shrink_0()
                    .px_6()
                    .gap_3()
                    .border_b_1()
                    .border_color(cx.api_outline_variant())
                    .bg(cx.api_surface_low())
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(if selected_is_active {
                                cx.api_primary_bright()
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(if selected_is_active {
                                "ACTIVE FOR REQUESTS"
                            } else {
                                "NOT ACTIVE"
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{variable_count} variables")),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                "Secret values are masked, but the local database is not encrypted.",
                            ),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_3()
                    .p_6()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_base().font_semibold().child("Variables"))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(
                                        "Reference enabled values in URLs, headers, bodies, and scripts.",
                                    ),
                            ),
                    )
                    .child(self.render_environment_variable_grid(cx)),
            )
            .into_any_element()
    }
}
