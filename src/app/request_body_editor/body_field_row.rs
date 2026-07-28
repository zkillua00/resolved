use super::super::*;

pub(in crate::app) struct BodyFieldRow {
    pub(in crate::app) id: usize,
    pub(in crate::app) name: Entity<InputState>,
    pub(in crate::app) value: Entity<InputState>,
    pub(in crate::app) enabled: bool,
    pub(in crate::app) kind: BodyFieldKind,
    pub(in crate::app) _subscriptions: Vec<Subscription>,
}

impl ApiTester {
    pub(super) fn render_body_field_row(
        &self,
        row: &BodyFieldRow,
        multipart: bool,
        this: gpui::WeakEntity<Self>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = row.id;
        let is_file = row.kind == BodyFieldKind::File;
        let selected_kind = row.kind;
        let kind_this = this.clone();
        let action_this = this.clone();
        let group_id: SharedString = format!("body-field-row-actions-{id}").into();
        let name_template_input = row.name.clone();
        let value_template_input = row.value.clone();

        h_flex()
            .id(("body-field-grid-row", id))
            .group(group_id.clone())
            .w_full()
            .h(px(46.))
            .flex_shrink_0()
            .border_t_1()
            .border_color(cx.api_outline_variant())
            .bg(cx.api_surface())
            .hover(|style| style.bg(cx.api_surface_low()))
            .when(!row.enabled, |this| this.opacity(0.55))
            .child(
                div()
                    .w(px(44.))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Checkbox::new(("body-field-enabled", id))
                            .checked(row.enabled)
                            .small()
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                if let Some(row) =
                                    this.body_fields.iter_mut().find(|row| row.id == id)
                                {
                                    row.enabled = *checked;
                                    this.refresh_request_dirty_part(
                                        RequestDirtyPart::BodyFields,
                                        cx,
                                    );
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .when(multipart, |this| {
                this.child(
                    div()
                        .w(px(96.))
                        .h_full()
                        .flex_shrink_0()
                        .border_l_1()
                        .border_color(cx.api_outline_variant())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Button::new(("body-field-kind", id))
                                .label(selected_kind.label())
                                .dropdown_caret(true)
                                .xsmall()
                                .ghost()
                                .w(px(82.))
                                .tooltip("Choose a text or file field")
                                .dropdown_menu(move |menu, _, _| {
                                    BodyFieldKind::all().iter().copied().fold(
                                        menu.min_w(px(150.)),
                                        |menu, kind| {
                                            let kind_this = kind_this.clone();
                                            menu.item(
                                                PopupMenuItem::new(kind.label())
                                                    .checked(kind == selected_kind)
                                                    .on_click(move |_, window, cx| {
                                                        if let Some(this) = kind_this.upgrade() {
                                                            this.update(cx, |this, cx| {
                                                                this.set_body_field_kind(
                                                                    id, kind, window, cx,
                                                                );
                                                            });
                                                        }
                                                    }),
                                            )
                                        },
                                    )
                                }),
                        ),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .border_l_1()
                    .border_color(cx.api_outline_variant())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event, window, cx| {
                            this.open_template_variable_popover(
                                name_template_input.clone(),
                                event,
                                window,
                                cx,
                            );
                        }),
                    )
                    .child(
                        Input::new(&row.name)
                            .appearance(false)
                            .small()
                            .size_full()
                            .px_3(),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .border_l_1()
                    .border_color(cx.api_outline_variant())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event, window, cx| {
                            this.open_template_variable_popover(
                                value_template_input.clone(),
                                event,
                                window,
                                cx,
                            );
                        }),
                    )
                    .child(
                        div().flex_1().min_w_0().h_full().child(
                            Input::new(&row.value)
                                .appearance(false)
                                .small()
                                .size_full()
                                .px_3(),
                        ),
                    )
                    .when(multipart && is_file, |this| {
                        this.child(
                            div().pr_2().child(
                                Button::new(("choose-body-file", id))
                                    .icon(IconName::FolderOpen)
                                    .label("Choose")
                                    .xsmall()
                                    .outline()
                                    .tooltip("Choose a local file")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.choose_body_file(id, window, cx);
                                    })),
                            ),
                        )
                    }),
            )
            .child(
                div()
                    .w(px(44.))
                    .h_full()
                    .flex_shrink_0()
                    .border_l_1()
                    .border_color(cx.api_outline_variant())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Button::new(("body-field-actions", id))
                            .icon(IconName::EllipsisVertical)
                            .xsmall()
                            .ghost()
                            .rounded_full()
                            .invisible()
                            .group_hover(group_id, |style| style.visible())
                            .tooltip("Field actions")
                            .dropdown_menu(move |menu, _, _| {
                                let duplicate_this = action_this.clone();
                                let remove_this = action_this.clone();
                                menu.item(PopupMenuItem::new("Duplicate").on_click(
                                    move |_, window, cx| {
                                        if let Some(this) = duplicate_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.duplicate_body_field_row(id, window, cx);
                                            });
                                        }
                                    },
                                ))
                                .separator()
                                .item(
                                    PopupMenuItem::new("Delete").on_click(move |_, window, cx| {
                                        if let Some(this) = remove_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.remove_body_field_row(id, window, cx);
                                            });
                                        }
                                    }),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }
}
