use super::*;
use crate::core::ManagementPermission;

const MANAGEMENT_SIDEBAR_WIDTH: f32 = 238.;

pub(super) fn render_user_management(this: &WeakEntity<ApiTester>, cx: &mut App) -> AnyElement {
    let Some(entity) = this.upgrade() else {
        return div().into_any_element();
    };
    let state = entity.read(cx);
    let management = state.server_management.clone();
    let active_upstream_id = state.settings.upstreams.active_upstream_id.clone();
    if let Some(status) = management_status_element(&management, active_upstream_id.as_deref(), cx)
    {
        return status;
    }
    let Some(snapshot) = management.snapshot.as_ref() else {
        return management_empty("User data is unavailable.", cx);
    };
    let Some(users) = snapshot.users.as_ref() else {
        return management_empty("You do not have permission to view users.", cx);
    };

    let roles = snapshot.roles.clone().unwrap_or_default();
    let selected = management
        .selected_user_id
        .as_ref()
        .and_then(|id| users.iter().find(|user| &user.id == id))
        .or_else(|| users.first());
    let busy = management.status.busy();
    let sidebar_rows = users
        .iter()
        .map(|user| {
            user_navigation_row(
                user,
                selected.is_some_and(|selected| selected.id == user.id),
                this,
                cx,
            )
        })
        .collect::<Vec<_>>();
    let sidebar = management_sidebar(
        "SERVER MEMBERS",
        format!("{} total", users.len()),
        "new-management-user",
        "New user",
        snapshot.has_permission(USERS_CREATE) && !busy,
        this.clone(),
        |app, window, cx| app.open_create_user_dialog(window, cx),
        sidebar_rows,
        cx,
    );
    let detail = match selected {
        Some(user) => render_user_detail(
            user,
            &roles,
            snapshot.has_permission(USERS_UPDATE),
            snapshot.has_permission(USERS_ASSIGN_ROLES),
            busy,
            this,
            cx,
        ),
        None => management_detail_empty("No users on this server.", cx),
    };
    management_split_panel(sidebar, detail)
}

fn user_navigation_row(
    user: &ManagementUser,
    selected: bool,
    this: &WeakEntity<ApiTester>,
    cx: &mut App,
) -> AnyElement {
    let select_this = this.clone();
    let user_id = user.id.clone();
    h_flex()
        .id(SharedString::from(format!(
            "select-management-user-{}",
            user.id
        )))
        .w_full()
        .gap_2()
        .px_2()
        .py_2()
        .rounded_md()
        .cursor_pointer()
        .when(selected, |row| row.bg(cx.theme().sidebar_accent))
        .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.62)))
        .on_click(move |_, _, cx| {
            if let Some(this) = select_this.upgrade() {
                this.update(cx, |this, cx| {
                    this.server_management.selected_user_id = Some(user_id.clone());
                    cx.notify();
                });
            }
        })
        .child(management_avatar(&user.display_name, cx))
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .gap_0p5()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .font_semibold()
                        .child(user.display_name.clone()),
                )
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(user.email.clone()),
                ),
        )
        .child(div().size(px(7.)).rounded_full().bg(if user.active {
            cx.theme().success
        } else {
            cx.theme().muted_foreground
        }))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_user_detail(
    user: &ManagementUser,
    roles: &[ManagementRole],
    can_update: bool,
    can_assign_roles: bool,
    busy: bool,
    this: &WeakEntity<ApiTester>,
    cx: &mut App,
) -> AnyElement {
    let edit_this = this.clone();
    let edit_user = user.clone();
    let active_this = this.clone();
    let active_user_id = user.id.clone();
    let next_active = !user.active;
    let assigned = user
        .roles
        .iter()
        .map(|role| role.id.clone())
        .collect::<BTreeSet<_>>();
    let mut role_rows = Vec::new();
    for role in roles {
        let checked = assigned.contains(&role.id);
        let mut next = assigned.clone();
        if checked {
            next.remove(&role.id);
        } else {
            next.insert(role.id.clone());
        }
        let action_this = this.clone();
        let action_user_id = user.id.clone();
        let action_role_ids = next.into_iter().collect::<Vec<_>>();
        role_rows.push(
            h_flex()
                .w_full()
                .gap_3()
                .px_3()
                .py_2p5()
                .border_b_1()
                .border_color(cx.api_outline_variant())
                .child(
                    v_flex()
                        .min_w_0()
                        .flex_1()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().text_sm().font_semibold().child(role.name.clone()))
                                .when(role.system, |row| {
                                    row.child(management_badge("System", cx.theme().info))
                                }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(if role.description.is_empty() {
                                    format!(
                                        "{} permission{}",
                                        role.permissions.len(),
                                        if role.permissions.len() == 1 { "" } else { "s" }
                                    )
                                } else {
                                    role.description.clone()
                                }),
                        ),
                )
                .child(
                    Switch::new(SharedString::from(format!(
                        "user-role-{}-{}",
                        user.id, role.id
                    )))
                    .checked(checked)
                    .disabled(!can_assign_roles || busy)
                    .on_click(move |_, window, cx| {
                        if let Some(this) = action_this.upgrade() {
                            this.update(cx, |this, cx| {
                                this.run_management_mutation(
                                    ManagementMutation::ReplaceUserRoles {
                                        user_id: action_user_id.clone(),
                                        role_ids: action_role_ids.clone(),
                                    },
                                    window,
                                    cx,
                                );
                            });
                        }
                    }),
                )
                .into_any_element(),
        );
    }

    v_flex()
        .h_full()
        .min_w_0()
        .child(
            h_flex()
                .w_full()
                .gap_3()
                .p_4()
                .border_b_1()
                .border_color(cx.api_outline_variant())
                .child(management_avatar_large(&user.display_name, cx))
                .child(
                    v_flex()
                        .min_w_0()
                        .flex_1()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .truncate()
                                        .text_lg()
                                        .font_semibold()
                                        .child(user.display_name.clone()),
                                )
                                .child(management_badge(
                                    if user.active { "Active" } else { "Inactive" },
                                    if user.active {
                                        cx.theme().success
                                    } else {
                                        cx.theme().muted_foreground
                                    },
                                )),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(user.email.clone()),
                        )
                        .child(creator_line(user.created_by.as_ref(), cx)),
                )
                .child(
                    Button::new(SharedString::from(format!("edit-user-detail-{}", user.id)))
                        .label("Edit profile")
                        .small()
                        .outline()
                        .disabled(!can_update || busy)
                        .on_click(move |_, window, cx| {
                            if let Some(this) = edit_this.upgrade() {
                                this.update(cx, |this, cx| {
                                    this.open_edit_user_dialog(edit_user.clone(), window, cx);
                                });
                            }
                        }),
                ),
        )
        .child(
            v_flex()
                .id(SharedString::from(format!(
                    "management-user-detail-scroll-{}",
                    user.id
                )))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p_4()
                .gap_5()
                .child(
                    v_flex()
                        .gap_2()
                        .child(management_section_title("ACCOUNT"))
                        .child(
                            h_flex()
                                .w_full()
                                .gap_3()
                                .py_3()
                                .border_y_1()
                                .border_color(cx.api_outline_variant())
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .gap_1()
                                        .child(div().text_sm().font_semibold().child("Enabled"))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child("Disabled users cannot log in."),
                                        ),
                                )
                                .child(
                                    Switch::new(SharedString::from(format!(
                                        "user-active-detail-{}",
                                        user.id
                                    )))
                                    .checked(user.active)
                                    .disabled(!can_update || busy)
                                    .on_click(
                                        move |_, window, cx| {
                                            if let Some(this) = active_this.upgrade() {
                                                this.update(cx, |this, cx| {
                                                    this.run_management_mutation(
                                                        ManagementMutation::UpdateUser {
                                                            user_id: active_user_id.clone(),
                                                            login: None,
                                                            display_name: None,
                                                            password: None,
                                                            active: Some(next_active),
                                                        },
                                                        window,
                                                        cx,
                                                    );
                                                });
                                            }
                                        },
                                    ),
                                ),
                        ),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(management_section_title("ROLES"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Roles determine this user's server-wide permissions."),
                        )
                        .child(if roles.is_empty() {
                            management_detail_empty(
                                if can_assign_roles {
                                    "Create a role before assigning one to this user."
                                } else {
                                    "Role assignments are not available."
                                },
                                cx,
                            )
                        } else {
                            v_flex()
                                .w_full()
                                .border_t_1()
                                .border_color(cx.api_outline_variant())
                                .children(role_rows)
                                .into_any_element()
                        }),
                ),
        )
        .into_any_element()
}

pub(super) fn render_role_management(this: &WeakEntity<ApiTester>, cx: &mut App) -> AnyElement {
    let Some(entity) = this.upgrade() else {
        return div().into_any_element();
    };
    let state = entity.read(cx);
    let management = state.server_management.clone();
    let active_upstream_id = state.settings.upstreams.active_upstream_id.clone();
    if let Some(status) = management_status_element(&management, active_upstream_id.as_deref(), cx)
    {
        return status;
    }
    let Some(snapshot) = management.snapshot.as_ref() else {
        return management_empty("Role data is unavailable.", cx);
    };
    let Some(roles) = snapshot.roles.as_ref() else {
        return management_empty("You do not have permission to view roles.", cx);
    };

    let selected = management
        .selected_role_id
        .as_ref()
        .and_then(|id| roles.iter().find(|role| &role.id == id))
        .or_else(|| roles.first());
    let busy = management.status.busy();
    let sidebar_rows = roles
        .iter()
        .map(|role| {
            role_navigation_row(
                role,
                selected.is_some_and(|selected| selected.id == role.id),
                this,
                cx,
            )
        })
        .collect::<Vec<_>>();
    let sidebar = management_sidebar(
        "SERVER ROLES",
        format!("{} total", roles.len()),
        "new-management-role",
        "New role",
        snapshot.has_permission(ROLES_CREATE) && !busy,
        this.clone(),
        |app, window, cx| app.open_create_role_dialog(window, cx),
        sidebar_rows,
        cx,
    );
    let permissions = snapshot.permissions.clone().unwrap_or_default();
    let detail = match selected {
        Some(role) => render_role_detail(
            role,
            &permissions,
            snapshot.has_permission(ROLES_UPDATE),
            snapshot.has_permission(ROLES_ASSIGN_PERMISSIONS),
            busy,
            this,
            cx,
        ),
        None => management_detail_empty("No roles on this server.", cx),
    };
    management_split_panel(sidebar, detail)
}

fn role_navigation_row(
    role: &ManagementRole,
    selected: bool,
    this: &WeakEntity<ApiTester>,
    cx: &mut App,
) -> AnyElement {
    let select_this = this.clone();
    let role_id = role.id.clone();
    h_flex()
        .id(SharedString::from(format!(
            "select-management-role-{}",
            role.id
        )))
        .w_full()
        .gap_2()
        .px_2()
        .py_2p5()
        .rounded_md()
        .cursor_pointer()
        .when(selected, |row| row.bg(cx.theme().sidebar_accent))
        .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.62)))
        .on_click(move |_, _, cx| {
            if let Some(this) = select_this.upgrade() {
                this.update(cx, |this, cx| {
                    this.server_management.selected_role_id = Some(role_id.clone());
                    cx.notify();
                });
            }
        })
        .child(div().size(px(10.)).rounded_full().bg(if role.system {
            cx.theme().info
        } else {
            cx.theme().accent
        }))
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .gap_0p5()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .font_semibold()
                        .child(role.name.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "{} permission{}",
                            role.permissions.len(),
                            if role.permissions.len() == 1 { "" } else { "s" }
                        )),
                ),
        )
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_role_detail(
    role: &ManagementRole,
    permissions: &[ManagementPermission],
    can_update: bool,
    can_assign: bool,
    busy: bool,
    this: &WeakEntity<ApiTester>,
    cx: &mut App,
) -> AnyElement {
    let edit_this = this.clone();
    let edit_role = role.clone();
    let assigned = role
        .permissions
        .iter()
        .map(|permission| permission.key.clone())
        .collect::<BTreeSet<_>>();
    let mut sorted_permissions = permissions.to_vec();
    sorted_permissions.sort_by(|left, right| left.key.cmp(&right.key));
    let mut permission_rows = Vec::new();
    let mut current_group = String::new();
    for permission in sorted_permissions {
        let group = permission
            .key
            .split_once('.')
            .map(|(group, _)| group)
            .unwrap_or(permission.key.as_str())
            .to_owned();
        if current_group != group {
            current_group = group.clone();
            permission_rows.push(
                div()
                    .w_full()
                    .px_3()
                    .pt_4()
                    .pb_2()
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(permission_group_title(&group))
                    .into_any_element(),
            );
        }
        let checked = assigned.contains(&permission.key);
        let mut next = assigned.clone();
        if checked {
            next.remove(&permission.key);
        } else {
            next.insert(permission.key.clone());
        }
        let action_this = this.clone();
        let action_role_id = role.id.clone();
        let action_keys = next.into_iter().collect::<Vec<_>>();
        permission_rows.push(
            h_flex()
                .w_full()
                .gap_3()
                .px_3()
                .py_2p5()
                .border_b_1()
                .border_color(cx.api_outline_variant())
                .child(
                    v_flex()
                        .min_w_0()
                        .flex_1()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .child(permission.key.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(permission.description.clone()),
                        ),
                )
                .child(
                    Switch::new(SharedString::from(format!(
                        "role-permission-{}-{}",
                        role.id, permission.key
                    )))
                    .checked(checked)
                    .disabled(role.system || !can_assign || busy)
                    .on_click(move |_, window, cx| {
                        if let Some(this) = action_this.upgrade() {
                            this.update(cx, |this, cx| {
                                this.run_management_mutation(
                                    ManagementMutation::ReplaceRolePermissions {
                                        role_id: action_role_id.clone(),
                                        permission_keys: action_keys.clone(),
                                    },
                                    window,
                                    cx,
                                );
                            });
                        }
                    }),
                )
                .into_any_element(),
        );
    }

    v_flex()
        .h_full()
        .min_w_0()
        .child(
            h_flex()
                .w_full()
                .gap_3()
                .p_4()
                .border_b_1()
                .border_color(cx.api_outline_variant())
                .child(
                    div()
                        .size(px(40.))
                        .rounded_full()
                        .bg(cx.theme().accent.opacity(0.16))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(div().size(px(14.)).rounded_full().bg(if role.system {
                            cx.theme().info
                        } else {
                            cx.theme().accent
                        })),
                )
                .child(
                    v_flex()
                        .min_w_0()
                        .flex_1()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .truncate()
                                        .text_lg()
                                        .font_semibold()
                                        .child(role.name.clone()),
                                )
                                .when(role.system, |row| {
                                    row.child(management_badge("System role", cx.theme().info))
                                }),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(if role.description.is_empty() {
                                    "No description".to_owned()
                                } else {
                                    role.description.clone()
                                }),
                        )
                        .child(creator_line(role.created_by.as_ref(), cx)),
                )
                .child(
                    Button::new(SharedString::from(format!("edit-role-detail-{}", role.id)))
                        .label("Edit role")
                        .small()
                        .outline()
                        .disabled(role.system || !can_update || busy)
                        .on_click(move |_, window, cx| {
                            if let Some(this) = edit_this.upgrade() {
                                this.update(cx, |this, cx| {
                                    this.open_edit_role_dialog(edit_role.clone(), window, cx);
                                });
                            }
                        }),
                ),
        )
        .child(
            v_flex()
                .id(SharedString::from(format!(
                    "management-role-detail-scroll-{}",
                    role.id
                )))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p_4()
                .gap_2()
                .child(management_section_title("PERMISSIONS"))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if role.system {
                            "System role permissions are managed by the server."
                        } else {
                            "Changes apply to every user assigned this role."
                        }),
                )
                .child(if permissions.is_empty() {
                    management_detail_empty("Permission details are not available.", cx)
                } else {
                    v_flex()
                        .w_full()
                        .border_t_1()
                        .border_color(cx.api_outline_variant())
                        .children(permission_rows)
                        .into_any_element()
                }),
        )
        .into_any_element()
}

pub(super) fn render_resource_management(this: &WeakEntity<ApiTester>, cx: &mut App) -> AnyElement {
    let Some(entity) = this.upgrade() else {
        return div().into_any_element();
    };
    let state = entity.read(cx);
    let management = state.server_management.clone();
    let active_upstream_id = state.settings.upstreams.active_upstream_id.clone();
    if let Some(status) = management_status_element(&management, active_upstream_id.as_deref(), cx)
    {
        return status;
    }
    let Some(snapshot) = management.snapshot.as_ref() else {
        return management_empty("Resource data is unavailable.", cx);
    };
    let Some(workspaces) = snapshot.workspaces.as_ref() else {
        return management_empty(
            "You do not have permission to view workspace resources.",
            cx,
        );
    };

    let selected = management
        .selected_resource
        .as_ref()
        .and_then(|selection| find_selected_resource(workspaces, selection))
        .or_else(|| workspaces.first().cloned().map(SelectedResource::Workspace));
    let mut sidebar_rows = Vec::new();
    for workspace in workspaces {
        append_workspace_navigation(
            &mut sidebar_rows,
            workspace,
            management.selected_resource.as_ref(),
            this,
            cx,
        );
    }
    let refresh_this = this.clone();
    let busy = management.status.busy();
    let sidebar = v_flex()
        .w(px(MANAGEMENT_SIDEBAR_WIDTH))
        .h_full()
        .flex_shrink_0()
        .border_r_1()
        .border_color(cx.api_outline_variant())
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .gap_2()
                .px_3()
                .py_3()
                .border_b_1()
                .border_color(cx.api_outline_variant())
                .child(
                    v_flex()
                        .gap_0p5()
                        .child(management_section_title("RESOURCES"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "{} workspace{}",
                                    workspaces.len(),
                                    if workspaces.len() == 1 { "" } else { "s" }
                                )),
                        ),
                )
                .child(
                    Button::new("refresh-management-resources")
                        .icon(IconName::Redo2)
                        .xsmall()
                        .ghost()
                        .disabled(busy)
                        .on_click(move |_, window, cx| {
                            if let Some(this) = refresh_this.upgrade() {
                                this.update(cx, |this, cx| {
                                    this.refresh_server_management(window, cx);
                                });
                            }
                        }),
                ),
        )
        .child(
            v_flex()
                .id("management-resource-navigation")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p_2()
                .gap_0p5()
                .children(sidebar_rows),
        )
        .into_any_element();
    let users = snapshot.users.clone().unwrap_or_default();
    let detail = match selected {
        Some(resource) => render_resource_detail(
            resource,
            &users,
            snapshot.has_permission(WORKSPACES_ASSIGN_USERS),
            snapshot.has_permission(COLLECTIONS_ASSIGN_USERS),
            busy,
            this,
            cx,
        ),
        None => management_detail_empty("No resources on this server.", cx),
    };
    management_split_panel(sidebar, detail)
}

#[derive(Clone)]
enum SelectedResource {
    Workspace(UpstreamWorkspaceView),
    Collection(UpstreamCollectionView),
    Request(UpstreamSavedRequestView),
}

pub(super) fn resource_selection_exists(
    workspaces: &[UpstreamWorkspaceView],
    selection: &ManagementResourceSelection,
) -> bool {
    find_selected_resource(workspaces, selection).is_some()
}

fn find_selected_resource(
    workspaces: &[UpstreamWorkspaceView],
    selection: &ManagementResourceSelection,
) -> Option<SelectedResource> {
    match selection {
        ManagementResourceSelection::Workspace(id) => workspaces
            .iter()
            .find(|workspace| &workspace.id == id)
            .cloned()
            .map(SelectedResource::Workspace),
        ManagementResourceSelection::Collection(id) => workspaces
            .iter()
            .find_map(|workspace| find_collection(&workspace.collections, id))
            .map(SelectedResource::Collection),
        ManagementResourceSelection::Request(id) => workspaces
            .iter()
            .find_map(|workspace| find_request(&workspace.collections, id))
            .map(SelectedResource::Request),
    }
}

fn find_collection(
    collections: &[UpstreamCollectionView],
    id: &str,
) -> Option<UpstreamCollectionView> {
    collections.iter().find_map(|collection| {
        if collection.id == id {
            Some(collection.clone())
        } else {
            find_collection(&collection.sub_collections, id)
        }
    })
}

fn find_request(
    collections: &[UpstreamCollectionView],
    id: &str,
) -> Option<UpstreamSavedRequestView> {
    collections.iter().find_map(|collection| {
        collection
            .requests
            .iter()
            .find(|request| request.id == id)
            .cloned()
            .or_else(|| find_request(&collection.sub_collections, id))
    })
}

fn append_workspace_navigation(
    rows: &mut Vec<AnyElement>,
    workspace: &UpstreamWorkspaceView,
    selected: Option<&ManagementResourceSelection>,
    this: &WeakEntity<ApiTester>,
    cx: &mut App,
) {
    let selection = ManagementResourceSelection::Workspace(workspace.id.clone());
    rows.push(resource_navigation_row(
        &workspace.name,
        "Workspace",
        0,
        selected == Some(&selection),
        selection,
        this,
        cx,
    ));
    append_collection_navigation(rows, &workspace.collections, 1, selected, this, cx);
}

fn append_collection_navigation(
    rows: &mut Vec<AnyElement>,
    collections: &[UpstreamCollectionView],
    depth: usize,
    selected: Option<&ManagementResourceSelection>,
    this: &WeakEntity<ApiTester>,
    cx: &mut App,
) {
    for collection in collections {
        let selection = ManagementResourceSelection::Collection(collection.id.clone());
        rows.push(resource_navigation_row(
            &collection.name,
            "Collection",
            depth,
            selected == Some(&selection),
            selection,
            this,
            cx,
        ));
        for request in &collection.requests {
            let selection = ManagementResourceSelection::Request(request.id.clone());
            rows.push(resource_navigation_row(
                &request.name,
                "Request",
                depth + 1,
                selected == Some(&selection),
                selection,
                this,
                cx,
            ));
        }
        append_collection_navigation(
            rows,
            &collection.sub_collections,
            depth + 1,
            selected,
            this,
            cx,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn resource_navigation_row(
    name: &str,
    kind: &str,
    depth: usize,
    selected: bool,
    selection: ManagementResourceSelection,
    this: &WeakEntity<ApiTester>,
    cx: &mut App,
) -> AnyElement {
    let row_this = this.clone();
    let id = match &selection {
        ManagementResourceSelection::Workspace(id) => format!("workspace-{id}"),
        ManagementResourceSelection::Collection(id) => format!("collection-{id}"),
        ManagementResourceSelection::Request(id) => format!("request-{id}"),
    };
    h_flex()
        .id(SharedString::from(format!(
            "select-management-resource-{id}"
        )))
        .w_full()
        .gap_2()
        .pl(px(8. + depth as f32 * 14.))
        .pr_2()
        .py_2()
        .rounded_md()
        .cursor_pointer()
        .when(selected, |row| row.bg(cx.theme().sidebar_accent))
        .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.62)))
        .on_click(move |_, _, cx| {
            if let Some(this) = row_this.upgrade() {
                this.update(cx, |this, cx| {
                    this.server_management.selected_resource = Some(selection.clone());
                    cx.notify();
                });
            }
        })
        .child(
            Icon::new(match kind {
                "Workspace" => IconName::Folder,
                "Collection" => IconName::FolderOpen,
                _ => IconName::File,
            })
            .size(px(14.))
            .text_color(cx.theme().muted_foreground),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_sm()
                .child(name.to_owned()),
        )
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_resource_detail(
    resource: SelectedResource,
    users: &[ManagementUser],
    can_assign_workspaces: bool,
    can_assign_collections: bool,
    busy: bool,
    this: &WeakEntity<ApiTester>,
    cx: &mut App,
) -> AnyElement {
    match resource {
        SelectedResource::Workspace(workspace) => render_access_detail(
            workspace.name,
            "Workspace",
            workspace.created_by,
            workspace.user_ids,
            AccessTarget::Workspace(workspace.id),
            users,
            can_assign_workspaces,
            busy,
            this,
            cx,
        ),
        SelectedResource::Collection(collection) => render_access_detail(
            collection.name,
            "Collection",
            collection.created_by,
            collection.user_ids,
            AccessTarget::Collection {
                workspace_id: collection.workspace_id,
                collection_id: collection.id,
            },
            users,
            can_assign_collections,
            busy,
            this,
            cx,
        ),
        SelectedResource::Request(request) => v_flex()
            .h_full()
            .min_w_0()
            .child(resource_detail_header(
                &request.name,
                "Request",
                request.created_by.as_ref(),
                cx,
            ))
            .child(
                v_flex()
                    .id(SharedString::from(format!(
                        "management-request-detail-scroll-{}",
                        request.id
                    )))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_4()
                    .gap_4()
                    .child(management_section_title("REQUEST"))
                    .child(
                        v_flex()
                            .w_full()
                            .gap_3()
                            .py_3()
                            .border_y_1()
                            .border_color(cx.api_outline_variant())
                            .child(management_metadata_row(
                                "Method",
                                request.definition.request.method,
                                cx,
                            ))
                            .child(management_metadata_row(
                                "URL",
                                if request.definition.request.url.is_empty() {
                                    "No URL".to_owned()
                                } else {
                                    request.definition.request.url
                                },
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("This request inherits access from its collection."),
                    ),
            )
            .into_any_element(),
    }
}

#[derive(Clone)]
enum AccessTarget {
    Workspace(String),
    Collection {
        workspace_id: String,
        collection_id: String,
    },
}

#[allow(clippy::too_many_arguments)]
fn render_access_detail(
    name: String,
    kind: &'static str,
    creator: Option<UpstreamUserSummary>,
    direct_user_ids: Vec<String>,
    target: AccessTarget,
    users: &[ManagementUser],
    can_assign: bool,
    busy: bool,
    this: &WeakEntity<ApiTester>,
    cx: &mut App,
) -> AnyElement {
    let assigned = direct_user_ids.into_iter().collect::<BTreeSet<_>>();
    let mut user_rows = Vec::new();
    for user in users {
        let checked = assigned.contains(&user.id);
        let mut next = assigned.clone();
        if checked {
            next.remove(&user.id);
        } else {
            next.insert(user.id.clone());
        }
        let action_user_ids = next.into_iter().collect::<Vec<_>>();
        let action_target = target.clone();
        let action_this = this.clone();
        user_rows.push(
            h_flex()
                .w_full()
                .gap_3()
                .px_3()
                .py_2p5()
                .border_b_1()
                .border_color(cx.api_outline_variant())
                .child(management_avatar(&user.display_name, cx))
                .child(
                    v_flex()
                        .min_w_0()
                        .flex_1()
                        .gap_0p5()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .truncate()
                                        .text_sm()
                                        .font_semibold()
                                        .child(user.display_name.clone()),
                                )
                                .when(!user.active, |row| {
                                    row.child(management_badge(
                                        "Inactive",
                                        cx.theme().muted_foreground,
                                    ))
                                }),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(user.email.clone()),
                        ),
                )
                .child(
                    Switch::new(SharedString::from(format!(
                        "resource-user-{}-{}",
                        resource_target_id(&target),
                        user.id
                    )))
                    .checked(checked)
                    .disabled(!can_assign || busy || (!user.active && !checked))
                    .on_click(move |_, window, cx| {
                        if let Some(this) = action_this.upgrade() {
                            this.update(cx, |this, cx| {
                                let mutation = match &action_target {
                                    AccessTarget::Workspace(workspace_id) => {
                                        ManagementMutation::ReplaceWorkspaceUsers {
                                            workspace_id: workspace_id.clone(),
                                            user_ids: action_user_ids.clone(),
                                        }
                                    }
                                    AccessTarget::Collection {
                                        workspace_id,
                                        collection_id,
                                    } => ManagementMutation::ReplaceCollectionUsers {
                                        workspace_id: workspace_id.clone(),
                                        collection_id: collection_id.clone(),
                                        user_ids: action_user_ids.clone(),
                                    },
                                };
                                this.run_management_mutation(mutation, window, cx);
                            });
                        }
                    }),
                )
                .into_any_element(),
        );
    }

    v_flex()
        .h_full()
        .min_w_0()
        .child(resource_detail_header(&name, kind, creator.as_ref(), cx))
        .child(
            v_flex()
                .id(SharedString::from(format!(
                    "management-access-detail-scroll-{}",
                    resource_target_id(&target)
                )))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p_4()
                .gap_2()
                .child(management_section_title("DIRECT ACCESS"))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if kind == "Workspace" {
                            "Members added here can access every collection in this workspace."
                        } else {
                            "Members added here can access this collection and its children."
                        }),
                )
                .child(if users.is_empty() {
                    management_detail_empty("User details are not available.", cx)
                } else {
                    v_flex()
                        .w_full()
                        .border_t_1()
                        .border_color(cx.api_outline_variant())
                        .children(user_rows)
                        .into_any_element()
                }),
        )
        .into_any_element()
}

fn resource_target_id(target: &AccessTarget) -> &str {
    match target {
        AccessTarget::Workspace(id) => id,
        AccessTarget::Collection { collection_id, .. } => collection_id,
    }
}

fn resource_detail_header(
    name: &str,
    kind: &'static str,
    creator: Option<&UpstreamUserSummary>,
    cx: &mut App,
) -> AnyElement {
    v_flex()
        .w_full()
        .gap_2()
        .p_4()
        .border_b_1()
        .border_color(cx.api_outline_variant())
        .child(
            h_flex()
                .gap_2()
                .child(div().text_lg().font_semibold().child(name.to_owned()))
                .child(management_badge(kind, cx.theme().muted_foreground)),
        )
        .child(creator_line(creator, cx))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn management_sidebar<F>(
    title: &'static str,
    count: String,
    button_id: &'static str,
    button_label: &'static str,
    enabled: bool,
    this: WeakEntity<ApiTester>,
    action: F,
    rows: Vec<AnyElement>,
    cx: &mut App,
) -> AnyElement
where
    F: Fn(&mut ApiTester, &mut Window, &mut Context<ApiTester>) + 'static,
{
    v_flex()
        .w(px(MANAGEMENT_SIDEBAR_WIDTH))
        .h_full()
        .flex_shrink_0()
        .border_r_1()
        .border_color(cx.api_outline_variant())
        .child(
            v_flex()
                .w_full()
                .gap_2()
                .p_3()
                .border_b_1()
                .border_color(cx.api_outline_variant())
                .child(
                    h_flex()
                        .justify_between()
                        .gap_2()
                        .child(management_section_title(title))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(count),
                        ),
                )
                .child(
                    Button::new(button_id)
                        .label(button_label)
                        .icon(IconName::Plus)
                        .small()
                        .primary()
                        .disabled(!enabled)
                        .on_click(move |_, window, cx| {
                            if let Some(this) = this.upgrade() {
                                this.update(cx, |this, cx| action(this, window, cx));
                            }
                        }),
                ),
        )
        .child(
            v_flex()
                .id(SharedString::from(format!(
                    "{}-management-navigation",
                    title.to_ascii_lowercase().replace(' ', "-")
                )))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p_2()
                .gap_0p5()
                .children(rows),
        )
        .into_any_element()
}

fn management_split_panel(sidebar: AnyElement, detail: AnyElement) -> AnyElement {
    h_flex()
        .w_full()
        .min_w(px(650.))
        .h_full()
        .min_h_0()
        .items_start()
        .overflow_hidden()
        .child(sidebar)
        .child(div().min_w_0().flex_1().h_full().child(detail))
        .into_any_element()
}

pub(super) fn management_detail_empty(
    message: impl Into<SharedString>,
    cx: &mut App,
) -> AnyElement {
    div()
        .w_full()
        .h_full()
        .min_h(px(120.))
        .flex()
        .items_center()
        .justify_center()
        .p_4()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(message.into())
        .into_any_element()
}

pub(super) fn management_section_title(title: &'static str) -> AnyElement {
    div()
        .text_size(px(11.))
        .font_semibold()
        .child(title)
        .into_any_element()
}

fn management_avatar(name: &str, cx: &mut App) -> AnyElement {
    div()
        .size(px(30.))
        .flex_shrink_0()
        .rounded_full()
        .bg(cx.theme().accent.opacity(0.16))
        .flex()
        .items_center()
        .justify_center()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().accent)
        .child(management_initial(name))
        .into_any_element()
}

fn management_avatar_large(name: &str, cx: &mut App) -> AnyElement {
    div()
        .size(px(46.))
        .flex_shrink_0()
        .rounded_full()
        .bg(cx.theme().accent.opacity(0.16))
        .flex()
        .items_center()
        .justify_center()
        .text_lg()
        .font_semibold()
        .text_color(cx.theme().accent)
        .child(management_initial(name))
        .into_any_element()
}

fn management_initial(name: &str) -> String {
    name.chars()
        .find(|character| !character.is_whitespace())
        .map(|character| character.to_uppercase().collect())
        .unwrap_or_else(|| "?".to_owned())
}

fn permission_group_title(group: &str) -> String {
    match group {
        "users" => "USER MANAGEMENT".to_owned(),
        "roles" => "ROLE MANAGEMENT".to_owned(),
        "permissions" => "PERMISSIONS".to_owned(),
        "workspaces" => "WORKSPACES".to_owned(),
        "collections" => "COLLECTIONS".to_owned(),
        "requests" => "REQUESTS".to_owned(),
        other => other.to_ascii_uppercase(),
    }
}

fn management_metadata_row(label: &'static str, value: String, cx: &mut App) -> AnyElement {
    h_flex()
        .w_full()
        .items_start()
        .gap_4()
        .child(
            div()
                .w(px(70.))
                .flex_shrink_0()
                .text_xs()
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(div().min_w_0().flex_1().text_sm().child(value))
        .into_any_element()
}
