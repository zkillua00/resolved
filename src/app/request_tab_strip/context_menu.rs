use super::super::*;

#[derive(Clone)]
struct TabMenuSnapshot {
    group_id: Option<RequestTabGroupId>,
    groups: Vec<RequestTabGroup>,
    close_others_disabled: bool,
    close_left_disabled: bool,
    close_right_disabled: bool,
}

#[derive(Clone)]
struct GroupMenuSnapshot {
    group_id: RequestTabGroupId,
    anchor_tab_id: RequestTabId,
    collapsed: bool,
    color: RequestTabGroupColor,
}

pub(super) fn build_request_tab_context_menu(
    menu: PopupMenu,
    owner: gpui::WeakEntity<ApiTester>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(entity) = owner.upgrade() else {
        return menu;
    };
    let target = entity.read(cx).request_tab_context_target.clone();
    match target {
        Some(super::RequestTabContextTarget::Tab(tab_id)) => {
            build_tab_menu(menu, owner, tab_id, window, cx)
        }
        Some(super::RequestTabContextTarget::Group(group_id)) => {
            build_group_menu(menu, owner, group_id, window, cx)
        }
        None => menu,
    }
}

fn build_tab_menu(
    mut menu: PopupMenu,
    owner: gpui::WeakEntity<ApiTester>,
    tab_id: RequestTabId,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(entity) = owner.upgrade() else {
        return menu;
    };
    let snapshot = {
        let app = entity.read(cx);
        let Some(index) = app
            .request_tabs
            .tabs()
            .iter()
            .position(|tab| tab.id() == &tab_id)
        else {
            return menu;
        };
        let tab = &app.request_tabs.tabs()[index];
        TabMenuSnapshot {
            group_id: tab.group_id().cloned(),
            groups: app.request_tabs.groups().to_vec(),
            close_others_disabled: app.request_tabs.tabs().len() < 2,
            close_left_disabled: index == 0,
            close_right_disabled: index + 1 >= app.request_tabs.tabs().len(),
        }
    };

    menu = menu
        .min_w(px(230.))
        .item(close_scope_item(
            "Close tab",
            false,
            owner.clone(),
            tab_id.clone(),
            RequestTabCloseScope::Current,
        ))
        .item(close_scope_item(
            "Close other tabs",
            snapshot.close_others_disabled,
            owner.clone(),
            tab_id.clone(),
            RequestTabCloseScope::Others,
        ))
        .separator()
        .item(close_scope_item(
            "Close tabs to the left",
            snapshot.close_left_disabled,
            owner.clone(),
            tab_id.clone(),
            RequestTabCloseScope::ToLeft,
        ))
        .item(close_scope_item(
            "Close tabs to the right",
            snapshot.close_right_disabled,
            owner.clone(),
            tab_id.clone(),
            RequestTabCloseScope::ToRight,
        ))
        .item(close_scope_item(
            "Close all tabs",
            false,
            owner.clone(),
            tab_id.clone(),
            RequestTabCloseScope::All,
        ))
        .separator();

    let groups = snapshot.groups.clone();
    let current_group_id = snapshot.group_id.clone();
    let group_owner = owner.clone();
    let group_tab_id = tab_id.clone();
    menu = menu.submenu("Move to group", window, cx, move |mut submenu, _, _| {
        submenu = submenu.max_h(px(420.)).scrollable(true);
        let create_owner = group_owner.clone();
        let create_tab_id = group_tab_id.clone();
        submenu = submenu.item(
            PopupMenuItem::new("New group…").on_click(move |_, window, cx| {
                let owner = create_owner.clone();
                let tab_id = create_tab_id.clone();
                window.defer(cx, move |window, cx| {
                    if let Some(owner) = owner.upgrade() {
                        owner.update(cx, |this, cx| {
                            this.open_new_request_tab_group_dialog(tab_id.clone(), window, cx);
                        });
                    }
                });
            }),
        );
        if !groups.is_empty() {
            submenu = submenu.separator();
        }
        for group in &groups {
            let assign_owner = group_owner.clone();
            let assign_tab_id = group_tab_id.clone();
            let assign_group_id = group.id().clone();
            let checked = current_group_id.as_ref() == Some(group.id());
            submenu = submenu.item(
                PopupMenuItem::new(compact_label(group.display_title(), 36))
                    .checked(checked)
                    .disabled(checked)
                    .on_click(move |_, window, cx| {
                        let owner = assign_owner.clone();
                        let tab_id = assign_tab_id.clone();
                        let group_id = assign_group_id.clone();
                        window.defer(cx, move |_, cx| {
                            if let Some(owner) = owner.upgrade() {
                                owner.update(cx, |this, cx| {
                                    this.assign_request_tab_to_group(
                                        tab_id.clone(),
                                        group_id.clone(),
                                        cx,
                                    );
                                });
                            }
                        });
                    }),
            );
        }
        submenu
    });

    if snapshot.group_id.is_some() {
        let remove_owner = owner.clone();
        let remove_tab_id = tab_id;
        menu = menu.item(
            PopupMenuItem::new("Remove from group").on_click(move |_, window, cx| {
                let owner = remove_owner.clone();
                let tab_id = remove_tab_id.clone();
                window.defer(cx, move |_, cx| {
                    if let Some(owner) = owner.upgrade() {
                        owner.update(cx, |this, cx| {
                            this.remove_request_tab_from_group(tab_id.clone(), cx);
                        });
                    }
                });
            }),
        );
    }

    menu
}

fn build_group_menu(
    mut menu: PopupMenu,
    owner: gpui::WeakEntity<ApiTester>,
    group_id: RequestTabGroupId,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(entity) = owner.upgrade() else {
        return menu;
    };
    let snapshot = {
        let app = entity.read(cx);
        let Some(group) = app.request_tabs.group(&group_id) else {
            return menu;
        };
        let Some(anchor_tab_id) = app
            .request_tabs
            .tabs_in_group(&group_id)
            .first()
            .map(|tab| tab.id().clone())
        else {
            return menu;
        };
        GroupMenuSnapshot {
            group_id: group_id.clone(),
            anchor_tab_id,
            collapsed: group.is_collapsed(),
            color: group.color().clone(),
        }
    };

    let new_tab_owner = owner.clone();
    let new_tab_group_id = snapshot.group_id.clone();
    let collapse_owner = owner.clone();
    let collapse_group_id = snapshot.group_id.clone();
    let rename_owner = owner.clone();
    let rename_group_id = snapshot.group_id.clone();
    menu = menu
        .min_w(px(220.))
        .item(
            PopupMenuItem::new("New tab in group").on_click(move |_, window, cx| {
                let owner = new_tab_owner.clone();
                let group_id = new_tab_group_id.clone();
                window.defer(cx, move |window, cx| {
                    if let Some(owner) = owner.upgrade() {
                        owner.update(cx, |this, cx| {
                            this.open_blank_request_tab_in_group(group_id.clone(), window, cx);
                        });
                    }
                });
            }),
        )
        .item(
            PopupMenuItem::new(if snapshot.collapsed {
                "Expand group"
            } else {
                "Collapse group"
            })
            .on_click(move |_, window, cx| {
                let owner = collapse_owner.clone();
                let group_id = collapse_group_id.clone();
                window.defer(cx, move |_, cx| {
                    if let Some(owner) = owner.upgrade() {
                        owner.update(cx, |this, cx| {
                            this.toggle_request_tab_group_collapsed(group_id.clone(), cx);
                        });
                    }
                });
            }),
        )
        .item(
            PopupMenuItem::new("Rename group…").on_click(move |_, window, cx| {
                let owner = rename_owner.clone();
                let group_id = rename_group_id.clone();
                window.defer(cx, move |window, cx| {
                    if let Some(owner) = owner.upgrade() {
                        owner.update(cx, |this, cx| {
                            this.open_rename_request_tab_group_dialog(group_id.clone(), window, cx);
                        });
                    }
                });
            }),
        );

    let color_owner = owner.clone();
    let color_group_id = snapshot.group_id.clone();
    let selected_color = snapshot.color.clone();
    menu = menu.submenu("Group color", window, cx, move |mut submenu, _, _| {
        for (label, color) in request_tab_group_colors() {
            let owner = color_owner.clone();
            let group_id = color_group_id.clone();
            let selected = selected_color == color;
            submenu = submenu.item(PopupMenuItem::new(label).checked(selected).on_click(
                move |_, window, cx| {
                    let owner = owner.clone();
                    let group_id = group_id.clone();
                    let color = color.clone();
                    window.defer(cx, move |_, cx| {
                        if let Some(owner) = owner.upgrade() {
                            owner.update(cx, |this, cx| {
                                this.set_request_tab_group_color(
                                    group_id.clone(),
                                    color.clone(),
                                    cx,
                                );
                            });
                        }
                    });
                },
            ));
        }
        submenu
    });

    let ungroup_owner = owner.clone();
    let ungroup_group_id = snapshot.group_id.clone();
    menu =
        menu.separator().item(
            PopupMenuItem::new("Ungroup tabs").on_click(move |_, window, cx| {
                let owner = ungroup_owner.clone();
                let group_id = ungroup_group_id.clone();
                window.defer(cx, move |_, cx| {
                    if let Some(owner) = owner.upgrade() {
                        owner.update(cx, |this, cx| {
                            this.remove_request_tab_group(group_id.clone(), cx);
                        });
                    }
                });
            }),
        );

    menu.item(close_scope_item(
        "Close group",
        false,
        owner,
        snapshot.anchor_tab_id,
        RequestTabCloseScope::Group,
    ))
}

fn close_scope_item(
    label: &'static str,
    disabled: bool,
    owner: gpui::WeakEntity<ApiTester>,
    anchor_id: RequestTabId,
    scope: RequestTabCloseScope,
) -> PopupMenuItem {
    PopupMenuItem::new(label)
        .disabled(disabled)
        .on_click(move |_, window, cx| {
            let owner = owner.clone();
            let anchor_id = anchor_id.clone();
            window.defer(cx, move |window, cx| {
                if let Some(owner) = owner.upgrade() {
                    owner.update(cx, |this, cx| {
                        this.request_close_request_tabs(anchor_id.clone(), scope, window, cx);
                    });
                }
            });
        })
}

fn request_tab_group_colors() -> Vec<(&'static str, RequestTabGroupColor)> {
    vec![
        ("Gray", RequestTabGroupColor::Gray),
        ("Blue", RequestTabGroupColor::Blue),
        ("Cyan", RequestTabGroupColor::Cyan),
        ("Green", RequestTabGroupColor::Green),
        ("Yellow", RequestTabGroupColor::Yellow),
        ("Orange", RequestTabGroupColor::Orange),
        ("Red", RequestTabGroupColor::Red),
        ("Pink", RequestTabGroupColor::Pink),
        ("Purple", RequestTabGroupColor::Purple),
    ]
}
