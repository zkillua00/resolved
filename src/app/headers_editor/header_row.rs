use super::*;

pub(in crate::app) struct HeaderRow {
    pub(in crate::app) id: usize,
    pub(in crate::app) name: Entity<InputState>,
    pub(in crate::app) value: Entity<InputState>,
    pub(in crate::app) enabled: bool,
    pub(in crate::app) _subscriptions: Vec<Subscription>,
}

impl ApiTester {
    pub(super) fn render_header_row(&self, row: &HeaderRow, cx: &mut Context<Self>) -> AnyElement {
        let id = row.id;
        let action_this = cx.entity().downgrade();
        let group_id: SharedString = format!("header-row-actions-{id}").into();
        let name_hover_input = row.name.clone();
        let name_click_input = row.name.clone();
        let value_hover_input = row.value.clone();
        let value_click_input = row.value.clone();
        let name_hover_this = action_this.clone();
        let value_hover_this = action_this.clone();
        let name_hover_input_id = row.name.entity_id();
        let value_hover_input_id = row.value.entity_id();

        h_flex()
            .id(("header-grid-row", id))
            .group(group_id.clone())
            .w_full()
            .h(px(44.))
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
                        Checkbox::new(("header-enabled", id))
                            .checked(row.enabled)
                            .small()
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                if let Some(row) = this.headers.iter_mut().find(|row| row.id == id)
                                {
                                    row.enabled = *checked;
                                    this.refresh_request_dirty_part(RequestDirtyPart::Headers, cx);
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .id(("header-name-template-source", id))
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .border_l_1()
                    .border_color(cx.api_outline_variant())
                    .on_hover(move |hovered, window, cx| {
                        if let Some(this) = name_hover_this.upgrade() {
                            this.update(cx, |this, cx| {
                                this.set_template_variable_source_hovered(
                                    name_hover_input_id,
                                    *hovered,
                                    window,
                                    cx,
                                );
                            });
                        }
                    })
                    .on_mouse_move(cx.listener(move |this, event, window, cx| {
                        this.hover_template_variable_popover(
                            name_hover_input.clone(),
                            event,
                            window,
                            cx,
                        );
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event, window, cx| {
                            this.open_template_variable_popover(
                                name_click_input.clone(),
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
                div()
                    .id(("header-value-template-source", id))
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .border_l_1()
                    .border_color(cx.api_outline_variant())
                    .on_hover(move |hovered, window, cx| {
                        if let Some(this) = value_hover_this.upgrade() {
                            this.update(cx, |this, cx| {
                                this.set_template_variable_source_hovered(
                                    value_hover_input_id,
                                    *hovered,
                                    window,
                                    cx,
                                );
                            });
                        }
                    })
                    .on_mouse_move(cx.listener(move |this, event, window, cx| {
                        this.hover_template_variable_popover(
                            value_hover_input.clone(),
                            event,
                            window,
                            cx,
                        );
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event, window, cx| {
                            this.open_template_variable_popover(
                                value_click_input.clone(),
                                event,
                                window,
                                cx,
                            );
                        }),
                    )
                    .child(
                        Input::new(&row.value)
                            .appearance(false)
                            .small()
                            .size_full()
                            .px_3(),
                    ),
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
                        Button::new(("header-actions", id))
                            .icon(IconName::EllipsisVertical)
                            .xsmall()
                            .ghost()
                            .rounded_full()
                            .invisible()
                            .group_hover(group_id, |style| style.visible())
                            .tooltip("Header actions")
                            .dropdown_menu(move |menu, _, _| {
                                let duplicate_this = action_this.clone();
                                let copy_this = action_this.clone();
                                let remove_this = action_this.clone();
                                menu.item(PopupMenuItem::new("Duplicate").on_click(
                                    move |_, window, cx| {
                                        if let Some(this) = duplicate_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.duplicate_header_row(id, window, cx);
                                            });
                                        }
                                    },
                                ))
                                .item(PopupMenuItem::new("Copy header").on_click(
                                    move |_, _, cx| {
                                        if let Some(this) = copy_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.copy_header_row(id, cx);
                                            });
                                        }
                                    },
                                ))
                                .separator()
                                .item(
                                    PopupMenuItem::new("Delete").on_click(move |_, window, cx| {
                                        if let Some(this) = remove_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.remove_header_row(id, window, cx);
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
