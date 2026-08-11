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
        can_update_definition: bool,
        can_update_values: bool,
        can_delete_definition: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let draft = row.id.starts_with("draft-variable-");
        let can_update_value = can_update_values || (draft && can_update_definition);
        let can_delete = can_delete_definition || (draft && can_update_definition);
        let checkbox_id = row.id.clone();
        let secret_id = row.id.clone();
        let action_this = cx.entity().downgrade();
        let context_this = action_this.clone();
        let actions_row_id = row.id.clone();
        let context_row_id = row.id.clone();
        let row_context_enabled = Rc::new(RefCell::new(true));
        let key_context_enabled = Rc::clone(&row_context_enabled);
        let value_context_enabled = Rc::clone(&row_context_enabled);
        let build_context_enabled = Rc::clone(&row_context_enabled);
        let group_id: SharedString = format!("environment-variable-actions-{}", row.id).into();
        let context_scope_id: SharedString =
            format!("environment-variable-context-menu-scope-{}", row.id).into();

        let row = h_flex()
            .id(("environment-variable-grid-row", index))
            .group(group_id.clone())
            .w_full()
            .h(px(46.))
            .flex_shrink_0()
            .border_t_1()
            .border_color(cx.api_outline_variant())
            .bg(cx.api_surface())
            .hover(|style| style.bg(cx.api_surface_low()))
            .when(!row.enabled, |this| this.opacity(0.58))
            .capture_any_mouse_down(move |event, _, _| {
                if event.button == MouseButton::Right {
                    *row_context_enabled.borrow_mut() = true;
                }
            })
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
                            .disabled(!can_update_definition)
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
                    .border_color(cx.api_outline_variant())
                    .capture_any_mouse_down(move |event, _, _| {
                        if event.button == MouseButton::Right {
                            *key_context_enabled.borrow_mut() = false;
                        }
                    })
                    .child(
                        Input::new(&row.key)
                            .appearance(false)
                            .small()
                            .size_full()
                            .px_3()
                            .disabled(!can_update_definition),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .border_l_1()
                    .border_color(cx.api_outline_variant())
                    .capture_any_mouse_down(move |event, _, _| {
                        if event.button == MouseButton::Right {
                            *value_context_enabled.borrow_mut() = false;
                        }
                    })
                    .child(
                        Input::new(&row.value)
                            .appearance(false)
                            .small()
                            .size_full()
                            .px_3()
                            .disabled(!can_update_value),
                    ),
            )
            .child(
                div()
                    .w(px(112.))
                    .h_full()
                    .flex_shrink_0()
                    .border_l_1()
                    .border_color(cx.api_outline_variant())
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
                            .disabled(!can_update_definition)
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
                    .border_color(cx.api_outline_variant())
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
                                build_environment_variable_actions_menu(
                                    menu,
                                    action_this.clone(),
                                    actions_row_id.clone(),
                                    can_update_definition,
                                    can_delete,
                                )
                            }),
                    ),
            )
            .context_menu(move |menu, _, _| {
                if !*build_context_enabled.borrow() {
                    return menu;
                }
                build_environment_variable_actions_menu(
                    menu,
                    context_this.clone(),
                    context_row_id.clone(),
                    can_update_definition,
                    can_delete,
                )
            });

        div()
            .id(context_scope_id)
            .w_full()
            .child(row)
            .into_any_element()
    }
}

fn build_environment_variable_actions_menu(
    menu: PopupMenu,
    owner: gpui::WeakEntity<ApiTester>,
    row_id: String,
    can_duplicate: bool,
    can_delete: bool,
) -> PopupMenu {
    let duplicate_this = owner.clone();
    let duplicate_id = row_id.clone();
    let remove_this = owner;
    let remove_id = row_id;
    menu.item(
        PopupMenuItem::new("Duplicate")
            .disabled(!can_duplicate)
            .on_click(move |_, window, cx| {
                if let Some(this) = duplicate_this.upgrade() {
                    this.update(cx, |this, cx| {
                        this.duplicate_environment_row(&duplicate_id, window, cx);
                    });
                }
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new("Delete")
            .disabled(!can_delete)
            .on_click(move |_, _, cx| {
                if let Some(this) = remove_this.upgrade() {
                    this.update(cx, |this, cx| {
                        this.environment_variables.retain(|row| row.id != remove_id);
                        cx.notify();
                    });
                }
            }),
    )
}
