use super::*;

impl ApiTester {
    pub(in crate::app) fn render_request_proxy_title_bar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .h(APP_TITLE_BAR_HEIGHT)
            .flex_shrink_0()
            .pl(window_chrome::leading_inset())
            .pr(window_chrome::trailing_inset())
            .border_b_1()
            .border_color(cx.theme().title_bar_border)
            .bg(cx.theme().title_bar)
            .child(
                h_flex().gap_6().child(resolved_brand_lockup(cx)).child(
                    h_flex()
                        .h_full()
                        .items_center()
                        .border_b_2()
                        .border_color(cx.theme().primary)
                        .px_1()
                        .text_sm()
                        .font_semibold()
                        .child("Request proxy"),
                ),
            )
            .child(window_chrome::caption_drag_region())
            .child(window_chrome::window_controls(window, cx))
            .into_any_element()
    }

    pub(in crate::app) fn render_request_proxy_workspace(
        &self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let management = &self.server_management;
        let active_upstream_id = self.settings.upstreams.active_upstream_id.clone();
        let server_label = self
            .settings
            .upstreams
            .active()
            .map(UpstreamProfile::display_label)
            .unwrap_or_else(|| "No server selected".to_owned());
        let busy = management.status.busy();
        let refresh_this = cx.entity().downgrade();

        let content = if let Some(status) = management_status_element(
            &management.status,
            management.upstream_id.as_deref(),
            management.snapshot.is_some(),
            active_upstream_id.as_deref(),
            cx,
        ) {
            v_flex()
                .w_full()
                .min_h(px(260.))
                .rounded_lg()
                .border_1()
                .border_color(cx.api_outline_variant())
                .bg(cx.api_surface_low())
                .child(status)
                .into_any_element()
        } else if let Some(snapshot) = management.snapshot.as_ref() {
            if snapshot.request_execution_settings.is_some() || snapshot.proxies.is_some() {
                render_request_proxy_settings(&cx.entity().downgrade(), management, snapshot, busy, cx)
            } else {
                request_proxy_empty(
                    "You do not have permission to view request proxy settings.",
                    cx,
                )
            }
        } else {
            request_proxy_empty("Request proxy settings are unavailable.", cx)
        };

        v_flex()
            .id("request-proxy-workspace")
            .debug_selector(|| "request-proxy-workspace".to_owned())
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            .when_some(self.settings_warning.clone(), |this, warning| {
                this.child(super::super::settings_page::dismissible_settings_message(
                    warning,
                    cx.theme().danger,
                    super::super::settings_page::SettingsMessageKind::Warning,
                    cx,
                ))
            })
            .when_some(self.settings_notice.clone(), |this, notice| {
                this.child(super::super::settings_page::dismissible_settings_message(
                    notice,
                    cx.theme().info,
                    super::super::settings_page::SettingsMessageKind::Notice,
                    cx,
                ))
            })
            .child(
                v_flex()
                    .id("request-proxy-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .items_center()
                    .p_6()
                    .child(
                        v_flex()
                            .w_full()
                            .max_w(px(1_080.))
                            .gap_6()
                            .child(
                                h_flex()
                                    .w_full()
                                    .items_start()
                                    .justify_between()
                                    .gap_4()
                                    .child(
                                        v_flex()
                                            .min_w_0()
                                            .flex_1()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_size(px(22.))
                                                    .font_semibold()
                                                    .child("Server request proxy"),
                                            )
                                            .child(
                                                div()
                                                    .max_w(px(720.))
                                                    .text_sm()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(
                                                        "Control where server-workspace requests run and how the server resolves request hostnames.",
                                                    ),
                                            )
                                            .child(
                                                h_flex()
                                                    .gap_2()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("Managing")
                                                    .child(
                                                        div()
                                                            .px_2()
                                                            .py_1()
                                                            .rounded_md()
                                                            .bg(cx.api_surface_low())
                                                            .font_semibold()
                                                            .text_color(cx.theme().foreground)
                                                            .child(server_label),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        Button::new("refresh-request-proxy-settings")
                                            .icon(IconName::Redo2)
                                            .label("Refresh")
                                            .small()
                                            .outline()
                                            .disabled(busy || active_upstream_id.is_none())
                                            .on_click(move |_, window, cx| {
                                                if let Some(this) = refresh_this.upgrade() {
                                                    this.update(cx, |this, cx| {
                                                        this.refresh_server_management(window, cx);
                                                    });
                                                }
                                            }),
                                    ),
                            )
                            .child(content)
                            .child(self.render_execution_limits(cx)),
                    ),
            )
            .into_any_element()
    }
}

fn render_request_proxy_settings(
    this: &WeakEntity<ApiTester>,
    management: &ServerManagementState,
    snapshot: &UpstreamManagementSnapshot,
    busy: bool,
    cx: &mut App,
) -> AnyElement {
    v_flex()
        .w_full()
        .gap_6()
        .child(match snapshot.request_execution_settings.as_ref() {
            Some(settings) => render_execution_location_card(
                this,
                settings,
                snapshot.has_permission(SERVER_SETTINGS_UPDATE),
                busy,
                cx,
            ),
            None => request_proxy_permission_note(
                "Viewing the execution location requires the server_settings.read permission.",
                cx,
            ),
        })
        .child(match snapshot.proxies.as_ref() {
            Some(proxies) => render_proxies_section(this, management, snapshot, proxies, busy, cx),
            None => request_proxy_permission_note(
                "Viewing proxies requires the proxies.read permission.",
                cx,
            ),
        })
        .child(
            h_flex()
                .w_full()
                .items_start()
                .gap_4()
                .child(request_proxy_behavior_card(
                    "Hostname to IP",
                    "The server connects to the IP while preserving the requested hostname for HTTP Host and HTTPS SNI. This behaves like a private DNS answer.",
                    cx,
                ))
                .child(request_proxy_behavior_card(
                    "Hostname to hostname",
                    "The target hostname becomes the outgoing URL host, HTTP Host, and HTTPS SNI. The original request hostname is not sent upstream.",
                    cx,
                ))
                .child(request_proxy_behavior_card(
                    "Scopes and exclusions",
                    "The most specific assigned proxy wins per hostname: request, then collection, then workspace, then server-wide. Excluded users and roles fall through to the next scope.",
                    cx,
                )),
        )
        .into_any_element()
}

fn render_execution_location_card(
    this: &WeakEntity<ApiTester>,
    settings: &RequestExecutionSettings,
    can_update: bool,
    busy: bool,
    cx: &mut App,
) -> AnyElement {
    let server_mode = settings.mode == RequestExecutionMode::Server;
    let mode_this = this.clone();
    let mode_settings = settings.clone();

    v_flex()
        .w_full()
        .rounded_lg()
        .border_1()
        .border_color(cx.api_outline_variant())
        .bg(cx.api_surface())
        .overflow_hidden()
        .child(
            h_flex()
                .w_full()
                .items_start()
                .justify_between()
                .gap_5()
                .p_5()
                .child(
                    v_flex()
                        .min_w_0()
                        .flex_1()
                        .gap_2()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .text_base()
                                        .font_semibold()
                                        .child("Execution location"),
                                )
                                .child(request_proxy_badge(
                                    if server_mode { "SERVER" } else { "LOCAL" },
                                    if server_mode {
                                        cx.theme().success
                                    } else {
                                        cx.theme().muted_foreground
                                    },
                                )),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(if server_mode {
                                    "Requests in every workspace on this server originate from the server. Assigned proxies apply before it connects."
                                } else {
                                    "Requests continue to originate from each user's computer. Proxies stay configured but are inactive."
                                }),
                        ),
                )
                .child(
                    v_flex()
                        .items_end()
                        .gap_2()
                        .child(
                            div()
                                .debug_selector(|| {
                                    "server-request-execution-enabled".to_owned()
                                })
                                .child(
                                    Switch::new("server-request-execution-enabled")
                                        .checked(server_mode)
                                        .disabled(!can_update || busy)
                                        .on_click(move |checked, window, cx| {
                                            if let Some(this) = mode_this.upgrade() {
                                                let mut settings = mode_settings.clone();
                                                settings.mode = if *checked {
                                                    RequestExecutionMode::Server
                                                } else {
                                                    RequestExecutionMode::Local
                                                };
                                                this.update(cx, |this, cx| {
                                                    this.run_management_mutation(
                                                        ManagementMutation::UpdateRequestExecutionSettings {
                                                            settings: settings.clone(),
                                                        },
                                                        window,
                                                        cx,
                                                    );
                                                });
                                            }
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(if server_mode { "Enabled" } else { "Disabled" }),
                        ),
                ),
        )
        .when(!can_update, |this| {
            this.child(
                div()
                    .w_full()
                    .px_5()
                    .py_3()
                    .border_t_1()
                    .border_color(cx.api_outline_variant())
                    .bg(cx.api_surface_low())
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("You can inspect this configuration, but only a server administrator can change it."),
            )
        })
        .into_any_element()
}

/// One assignable scope node, flattened from the workspace trees with the
/// server-wide scope first.
struct ProxyScopeOption {
    kind: ProxyScopeKind,
    scope_id: Option<String>,
    label: String,
    detail: String,
    depth: usize,
}

fn proxy_scope_options(workspaces: Option<&[UpstreamWorkspaceView]>) -> Vec<ProxyScopeOption> {
    let mut options = vec![ProxyScopeOption {
        kind: ProxyScopeKind::Server,
        scope_id: None,
        label: "Entire server".to_owned(),
        detail: "Every workspace on this server".to_owned(),
        depth: 0,
    }];
    for workspace in workspaces.unwrap_or_default() {
        options.push(ProxyScopeOption {
            kind: ProxyScopeKind::Workspace,
            scope_id: Some(workspace.id.clone()),
            label: workspace.name.clone(),
            detail: "Workspace".to_owned(),
            depth: 0,
        });
        for collection in &workspace.collections {
            push_collection_scope_options(&mut options, collection, &workspace.name, 1);
        }
    }
    options
}

fn push_collection_scope_options(
    options: &mut Vec<ProxyScopeOption>,
    collection: &UpstreamCollectionView,
    path: &str,
    depth: usize,
) {
    options.push(ProxyScopeOption {
        kind: ProxyScopeKind::Collection,
        scope_id: Some(collection.id.clone()),
        label: collection.name.clone(),
        detail: format!("Collection · {path}"),
        depth,
    });
    let nested_path = format!("{path} / {}", collection.name);
    for request in &collection.requests {
        options.push(ProxyScopeOption {
            kind: ProxyScopeKind::Request,
            scope_id: Some(request.id.clone()),
            label: request.name.clone(),
            detail: format!("Request · {nested_path}"),
            depth: depth + 1,
        });
    }
    for sub_collection in &collection.sub_collections {
        push_collection_scope_options(options, sub_collection, &nested_path, depth + 1);
    }
}

fn scope_owner_key(kind: ProxyScopeKind, scope_id: Option<&str>) -> (ProxyScopeKind, String) {
    (kind, scope_id.unwrap_or_default().to_owned())
}

fn proxy_scope_kind_noun(kind: ProxyScopeKind) -> &'static str {
    match kind {
        ProxyScopeKind::Server => "server",
        ProxyScopeKind::Workspace => "workspace",
        ProxyScopeKind::Collection => "collection",
        ProxyScopeKind::Request => "request",
    }
}

fn proxy_assignment_label(
    assignment: &ProxyAssignment,
    options: &[ProxyScopeOption],
) -> String {
    if assignment.scope_kind == ProxyScopeKind::Server {
        return "Server-wide".to_owned();
    }
    options
        .iter()
        .find(|option| {
            option.kind == assignment.scope_kind
                && option.scope_id.as_deref() == assignment.scope_id.as_deref()
        })
        .map(|option| {
            format!(
                "{} {}",
                capitalized_scope_noun(assignment.scope_kind),
                option.label
            )
        })
        .unwrap_or_else(|| {
            format!(
                "Inaccessible {}",
                proxy_scope_kind_noun(assignment.scope_kind)
            )
        })
}

fn capitalized_scope_noun(kind: ProxyScopeKind) -> &'static str {
    match kind {
        ProxyScopeKind::Server => "Server",
        ProxyScopeKind::Workspace => "Workspace:",
        ProxyScopeKind::Collection => "Collection:",
        ProxyScopeKind::Request => "Request:",
    }
}

#[allow(clippy::too_many_arguments)]
fn render_proxies_section(
    this: &WeakEntity<ApiTester>,
    management: &ServerManagementState,
    snapshot: &UpstreamManagementSnapshot,
    proxies: &[ManagementProxy],
    busy: bool,
    cx: &mut App,
) -> AnyElement {
    let can_create = snapshot.has_permission(PROXIES_CREATE);
    let add_this = this.clone();
    let scope_options = Rc::new(proxy_scope_options(snapshot.workspaces.as_deref()));
    let mut scope_owners: BTreeMap<(ProxyScopeKind, String), (String, String)> = BTreeMap::new();
    for proxy in proxies {
        for assignment in &proxy.assignments {
            scope_owners.insert(
                scope_owner_key(assignment.scope_kind, assignment.scope_id.as_deref()),
                (proxy.id.clone(), proxy.name.clone()),
            );
        }
    }
    let scope_owners = Rc::new(scope_owners);

    let cards = proxies
        .iter()
        .map(|proxy| {
            render_proxy_card(
                this,
                management,
                snapshot,
                proxy,
                &scope_options,
                &scope_owners,
                busy,
                cx,
            )
        })
        .collect::<Vec<_>>();

    v_flex()
        .w_full()
        .gap_4()
        .child(
            v_flex()
                .w_full()
                .rounded_lg()
                .border_1()
                .border_color(cx.api_outline_variant())
                .bg(cx.api_surface())
                .overflow_hidden()
                .child(
                    h_flex()
                        .w_full()
                        .items_start()
                        .justify_between()
                        .gap_5()
                        .p_5()
                        .child(
                            v_flex()
                                .min_w_0()
                                .flex_1()
                                .gap_2()
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(div().text_base().font_semibold().child("Proxies"))
                                        .child(request_proxy_badge(
                                            format!("{} PROXIES", cards.len()),
                                            cx.theme().primary,
                                        )),
                                )
                                .child(
                                    div()
                                        .max_w(px(720.))
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(
                                            "Group host override rules into named proxies, then assign each proxy server-wide or to a workspace, collection, or request. Opt users or roles out of a proxy to make their requests fall through to the next scope.",
                                        ),
                                ),
                        )
                        .child(
                            div().debug_selector(|| "new-proxy".to_owned()).child(
                                Button::new("new-proxy")
                                    .icon(IconName::Plus)
                                    .label("New proxy")
                                    .small()
                                    .primary()
                                    .disabled(!can_create || busy)
                                    .on_click(move |_, window, cx| {
                                        if let Some(this) = add_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.open_proxy_name_dialog(None, window, cx);
                                            });
                                        }
                                    }),
                            ),
                        ),
                )
                .when(cards.is_empty(), |this| {
                    this.child(
                        v_flex()
                            .w_full()
                            .min_h(px(140.))
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .p_6()
                            .border_t_1()
                            .border_color(cx.api_outline_variant())
                            .child(div().text_sm().font_semibold().child("No proxies"))
                            .child(
                                div()
                                    .max_w(px(480.))
                                    .text_center()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Requests use normal DNS resolution until a proxy with host override rules is assigned."),
                            ),
                    )
                }),
        )
        .children(cards)
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_proxy_card(
    this: &WeakEntity<ApiTester>,
    management: &ServerManagementState,
    snapshot: &UpstreamManagementSnapshot,
    proxy: &ManagementProxy,
    scope_options: &Rc<Vec<ProxyScopeOption>>,
    scope_owners: &Rc<BTreeMap<(ProxyScopeKind, String), (String, String)>>,
    busy: bool,
    cx: &mut App,
) -> AnyElement {
    let can_update = snapshot.has_permission(PROXIES_UPDATE);
    let can_delete = snapshot.has_permission(PROXIES_DELETE);
    let can_assign = snapshot.has_permission(PROXIES_ASSIGN);

    let rename_this = this.clone();
    let rename_proxy = proxy.clone();
    let delete_this = this.clone();
    let delete_proxy_id = proxy.id.clone();
    let delete_proxy_name = proxy.name.clone();
    let add_rule_this = this.clone();
    let add_rule_proxy_id = proxy.id.clone();

    let rule_rows = proxy
        .rules
        .iter()
        .map(|entry| render_proxy_rule_row(this, &proxy.id, entry, can_update, busy, cx))
        .collect::<Vec<_>>();

    let card_selector = format!("request-proxy-card-{}", proxy.id);
    let exclusion_count = proxy.excluded_user_ids.len() + proxy.excluded_role_ids.len();

    v_flex()
        .debug_selector(move || card_selector.clone())
        .w_full()
        .rounded_lg()
        .border_1()
        .border_color(cx.api_outline_variant())
        .bg(cx.api_surface())
        .overflow_hidden()
        .child(
            h_flex()
                .w_full()
                .items_start()
                .justify_between()
                .gap_5()
                .p_5()
                .child(
                    v_flex()
                        .min_w_0()
                        .flex_1()
                        .gap_2()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().text_base().font_semibold().child(proxy.name.clone()))
                                .child(request_proxy_badge(
                                    format!("{} RULES", proxy.rules.len()),
                                    cx.theme().primary,
                                ))
                                .when(exclusion_count > 0, |row| {
                                    row.child(request_proxy_badge(
                                        format!("{exclusion_count} EXCLUDED"),
                                        cx.theme().warning,
                                    ))
                                }),
                        )
                        .child(render_proxy_assignment_chips(proxy, scope_options, cx)),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new(SharedString::from(format!("rename-proxy-{}", proxy.id)))
                                .icon(IconName::Settings2)
                                .label("Rename")
                                .xsmall()
                                .outline()
                                .disabled(!can_update || busy)
                                .on_click(move |_, window, cx| {
                                    if let Some(this) = rename_this.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.open_proxy_name_dialog(
                                                Some(rename_proxy.clone()),
                                                window,
                                                cx,
                                            );
                                        });
                                    }
                                }),
                        )
                        .child(
                            Button::new(SharedString::from(format!("delete-proxy-{}", proxy.id)))
                                .icon(IconName::Delete)
                                .label("Delete")
                                .xsmall()
                                .ghost()
                                .danger()
                                .disabled(!can_delete || busy)
                                .on_click(move |_, window, cx| {
                                    if let Some(this) = delete_this.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.request_delete_proxy(
                                                delete_proxy_id.clone(),
                                                delete_proxy_name.clone(),
                                                window,
                                                cx,
                                            );
                                        });
                                    }
                                }),
                        ),
                ),
        )
        .child(
            h_flex()
                .w_full()
                .gap_4()
                .px_5()
                .py_2()
                .border_y_1()
                .border_color(cx.api_outline_variant())
                .bg(cx.api_surface_low())
                .text_size(px(10.))
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child(div().w(px(220.)).child("REQUEST HOST"))
                .child(div().w(px(220.)).child("CONNECT TO / SCHEME"))
                .child(div().min_w_0().flex_1().child("ORIGIN BEHAVIOR"))
                .child(div().w(px(152.)).text_right().child("ACTIONS")),
        )
        .child(if rule_rows.is_empty() {
            v_flex()
                .w_full()
                .items_center()
                .justify_center()
                .gap_1()
                .p_5()
                .child(div().text_sm().font_semibold().child("No rules yet"))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("This proxy has no effect until a host override rule is added."),
                )
                .into_any_element()
        } else {
            v_flex().w_full().children(rule_rows).into_any_element()
        })
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .gap_2()
                .px_5()
                .py_3()
                .border_t_1()
                .border_color(cx.api_outline_variant())
                .child(
                    Button::new(SharedString::from(format!("add-proxy-rule-{}", proxy.id)))
                        .icon(IconName::Plus)
                        .label("Add rule")
                        .xsmall()
                        .outline()
                        .disabled(!can_update || busy)
                        .on_click(move |_, window, cx| {
                            if let Some(this) = add_rule_this.upgrade() {
                                this.update(cx, |this, cx| {
                                    this.open_proxy_rule_dialog(
                                        add_rule_proxy_id.clone(),
                                        None,
                                        window,
                                        cx,
                                    );
                                });
                            }
                        }),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .child(render_proxy_panel_toggle(
                            this,
                            proxy,
                            ProxyEditorPanel::Assignments,
                            management
                                .expanded_proxy_assignments
                                .contains(&proxy.id),
                            can_assign,
                            busy,
                        ))
                        .child(render_proxy_panel_toggle(
                            this,
                            proxy,
                            ProxyEditorPanel::Exclusions,
                            management.expanded_proxy_exclusions.contains(&proxy.id),
                            can_assign,
                            busy,
                        )),
                ),
        )
        .when(
            management.expanded_proxy_assignments.contains(&proxy.id),
            |card| {
                card.child(render_proxy_assignment_editor(
                    this,
                    proxy,
                    scope_options,
                    scope_owners,
                    can_assign,
                    busy,
                    cx,
                ))
            },
        )
        .when(
            management.expanded_proxy_exclusions.contains(&proxy.id),
            |card| {
                card.child(render_proxy_exclusion_editor(
                    this, snapshot, proxy, can_assign, busy, cx,
                ))
            },
        )
        .into_any_element()
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum ProxyEditorPanel {
    Assignments,
    Exclusions,
}

fn render_proxy_panel_toggle(
    this: &WeakEntity<ApiTester>,
    proxy: &ManagementProxy,
    panel: ProxyEditorPanel,
    expanded: bool,
    can_assign: bool,
    busy: bool,
) -> AnyElement {
    let (id_prefix, label) = match panel {
        ProxyEditorPanel::Assignments => ("manage-proxy-assignments", "Assignments"),
        ProxyEditorPanel::Exclusions => ("manage-proxy-exclusions", "Exclusions"),
    };
    let toggle_this = this.clone();
    let proxy_id = proxy.id.clone();
    let selector = format!("{id_prefix}-{}", proxy.id);
    let button = Button::new(SharedString::from(selector.clone()))
        .label(if expanded {
            format!("Hide {}", label.to_ascii_lowercase())
        } else {
            format!("Manage {}", label.to_ascii_lowercase())
        })
        .xsmall()
        .outline()
        .disabled(!can_assign || busy)
        .on_click(move |_, _, cx| {
            if let Some(this) = toggle_this.upgrade() {
                this.update(cx, |this, cx| match panel {
                    ProxyEditorPanel::Assignments => {
                        this.toggle_proxy_assignments_editor(&proxy_id, cx);
                    }
                    ProxyEditorPanel::Exclusions => {
                        this.toggle_proxy_exclusions_editor(&proxy_id, cx);
                    }
                });
            }
        });
    div()
        .debug_selector(move || selector.clone())
        .child(button)
        .into_any_element()
}

fn render_proxy_assignment_chips(
    proxy: &ManagementProxy,
    scope_options: &[ProxyScopeOption],
    cx: &mut App,
) -> AnyElement {
    if proxy.assignments.is_empty() {
        return div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child("Not assigned anywhere — this proxy is inactive.")
            .into_any_element();
    }
    h_flex()
        .flex_wrap()
        .gap_1()
        .children(proxy.assignments.iter().map(|assignment| {
            let label = proxy_assignment_label(assignment, scope_options);
            div()
                .px_2()
                .py_1()
                .rounded_md()
                .bg(cx.api_surface_low())
                .border_1()
                .border_color(cx.api_outline_variant())
                .text_xs()
                .child(label)
        }))
        .into_any_element()
}

fn render_proxy_assignment_editor(
    this: &WeakEntity<ApiTester>,
    proxy: &ManagementProxy,
    scope_options: &Rc<Vec<ProxyScopeOption>>,
    scope_owners: &Rc<BTreeMap<(ProxyScopeKind, String), (String, String)>>,
    can_assign: bool,
    busy: bool,
    cx: &mut App,
) -> AnyElement {
    let assignments = Rc::new(proxy.assignments.clone());
    let rows = scope_options
        .iter()
        .map(|option| {
            let assigned_here = assignments.iter().any(|assignment| {
                assignment.scope_kind == option.kind
                    && assignment.scope_id.as_deref() == option.scope_id.as_deref()
            });
            let owner = scope_owners
                .get(&scope_owner_key(option.kind, option.scope_id.as_deref()))
                .filter(|(owner_id, _)| owner_id != &proxy.id);
            let taken_by = owner.map(|(_, name)| name.clone());
            let toggle_this = this.clone();
            let toggle_proxy_id = proxy.id.clone();
            let toggle_assignments = assignments.clone();
            let toggle_assignment = ProxyAssignment {
                scope_kind: option.kind,
                scope_id: option.scope_id.clone(),
            };
            let scope_selector = format!(
                "proxy-scope-{}-{}",
                proxy.id,
                option.scope_id.as_deref().unwrap_or("server")
            );
            h_flex()
                .debug_selector({
                    let scope_selector = scope_selector.clone();
                    move || scope_selector.clone()
                })
                .w_full()
                .gap_3()
                .px_5()
                .py_2p5()
                .border_t_1()
                .border_color(cx.api_outline_variant())
                .child(
                    v_flex()
                        .min_w_0()
                        .flex_1()
                        .pl(px(16. * option.depth as f32))
                        .gap_0p5()
                        .child(div().text_sm().font_semibold().child(option.label.clone()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(match taken_by.as_deref() {
                                    Some(other) => format!("{} · assigned to {other}", option.detail),
                                    None => option.detail.clone(),
                                }),
                        ),
                )
                .child(
                    Switch::new(SharedString::from(scope_selector))
                        .checked(assigned_here)
                        .disabled(!can_assign || busy || taken_by.is_some())
                        .on_click(move |checked, window, cx| {
                            if let Some(this) = toggle_this.upgrade() {
                                let mut next = toggle_assignments.as_ref().clone();
                                next.retain(|assignment| assignment != &toggle_assignment);
                                if *checked {
                                    next.push(toggle_assignment.clone());
                                }
                                this.update(cx, |this, cx| {
                                    this.run_management_mutation(
                                        ManagementMutation::ReplaceProxyAssignments {
                                            proxy_id: toggle_proxy_id.clone(),
                                            assignments: next,
                                        },
                                        window,
                                        cx,
                                    );
                                });
                            }
                        }),
                )
                .into_any_element()
        })
        .collect::<Vec<_>>();

    v_flex()
        .w_full()
        .child(
            div()
                .w_full()
                .px_5()
                .py_2()
                .border_t_1()
                .border_color(cx.api_outline_variant())
                .bg(cx.api_surface_low())
                .text_size(px(10.))
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child("ASSIGN THIS PROXY TO"),
        )
        .children(rows)
        .into_any_element()
}

fn render_proxy_exclusion_editor(
    this: &WeakEntity<ApiTester>,
    snapshot: &UpstreamManagementSnapshot,
    proxy: &ManagementProxy,
    can_assign: bool,
    busy: bool,
    cx: &mut App,
) -> AnyElement {
    let excluded_users = Rc::new(proxy.excluded_user_ids.clone());
    let excluded_roles = Rc::new(proxy.excluded_role_ids.clone());

    let mut rows: Vec<AnyElement> = Vec::new();
    if let Some(roles) = snapshot.roles.as_ref() {
        for role in roles {
            rows.push(render_proxy_exclusion_row(
                this,
                proxy,
                &excluded_users,
                &excluded_roles,
                ProxyExclusionSubject::Role,
                &role.id,
                &role.name,
                "Role",
                can_assign,
                busy,
                cx,
            ));
        }
    }
    if let Some(users) = snapshot.users.as_ref() {
        for user in users {
            rows.push(render_proxy_exclusion_row(
                this,
                proxy,
                &excluded_users,
                &excluded_roles,
                ProxyExclusionSubject::User,
                &user.id,
                &user.display_name,
                "User",
                can_assign,
                busy,
                cx,
            ));
        }
    }

    v_flex()
        .w_full()
        .child(
            div()
                .w_full()
                .px_5()
                .py_2()
                .border_t_1()
                .border_color(cx.api_outline_variant())
                .bg(cx.api_surface_low())
                .text_size(px(10.))
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child("OPT OUT USERS AND ROLES"),
        )
        .child(
            div()
                .w_full()
                .px_5()
                .py_2()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(
                    "Excluded users and roles skip this proxy; their requests fall through to the next scope or normal DNS resolution. Everyone else with access uses it automatically.",
                ),
        )
        .child(if rows.is_empty() {
            div()
                .w_full()
                .px_5()
                .py_3()
                .border_t_1()
                .border_color(cx.api_outline_variant())
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("Viewing users and roles requires the users.read and roles.read permissions.")
                .into_any_element()
        } else {
            v_flex().w_full().children(rows).into_any_element()
        })
        .into_any_element()
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum ProxyExclusionSubject {
    User,
    Role,
}

#[allow(clippy::too_many_arguments)]
fn render_proxy_exclusion_row(
    this: &WeakEntity<ApiTester>,
    proxy: &ManagementProxy,
    excluded_users: &Rc<Vec<String>>,
    excluded_roles: &Rc<Vec<String>>,
    subject: ProxyExclusionSubject,
    subject_id: &str,
    subject_label: &str,
    subject_detail: &'static str,
    can_assign: bool,
    busy: bool,
    cx: &mut App,
) -> AnyElement {
    let excluded = match subject {
        ProxyExclusionSubject::User => excluded_users.iter().any(|id| id == subject_id),
        ProxyExclusionSubject::Role => excluded_roles.iter().any(|id| id == subject_id),
    };
    let toggle_this = this.clone();
    let toggle_proxy_id = proxy.id.clone();
    let toggle_subject_id = subject_id.to_owned();
    let toggle_users = excluded_users.clone();
    let toggle_roles = excluded_roles.clone();
    let subject_kind = match subject {
        ProxyExclusionSubject::User => "user",
        ProxyExclusionSubject::Role => "role",
    };
    let selector = format!("proxy-exclusion-{}-{subject_kind}-{subject_id}", proxy.id);

    h_flex()
        .debug_selector({
            let selector = selector.clone();
            move || selector.clone()
        })
        .w_full()
        .gap_3()
        .px_5()
        .py_2p5()
        .border_t_1()
        .border_color(cx.api_outline_variant())
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .gap_0p5()
                .child(div().text_sm().font_semibold().child(subject_label.to_owned()))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if excluded {
                            format!("{subject_detail} · excluded from this proxy")
                        } else {
                            format!("{subject_detail} · uses this proxy")
                        }),
                ),
        )
        .child(
            Switch::new(SharedString::from(selector))
                .checked(excluded)
                .disabled(!can_assign || busy)
                .on_click(move |checked, window, cx| {
                    if let Some(this) = toggle_this.upgrade() {
                        let mut users = toggle_users.as_ref().clone();
                        let mut roles = toggle_roles.as_ref().clone();
                        let list = match subject {
                            ProxyExclusionSubject::User => &mut users,
                            ProxyExclusionSubject::Role => &mut roles,
                        };
                        list.retain(|id| id != &toggle_subject_id);
                        if *checked {
                            list.push(toggle_subject_id.clone());
                        }
                        this.update(cx, |this, cx| {
                            this.run_management_mutation(
                                ManagementMutation::ReplaceProxyExclusions {
                                    proxy_id: toggle_proxy_id.clone(),
                                    excluded_user_ids: users,
                                    excluded_role_ids: roles,
                                },
                                window,
                                cx,
                            );
                        });
                    }
                }),
        )
        .into_any_element()
}

fn render_proxy_rule_row(
    this: &WeakEntity<ApiTester>,
    proxy_id: &str,
    entry: &HostnameOverride,
    can_update: bool,
    busy: bool,
    cx: &mut App,
) -> AnyElement {
    let parsed_target = url::Url::parse(&entry.target).ok().filter(|target| {
        matches!(target.scheme(), "http" | "https") && target.host_str().is_some()
    });
    let target_host = parsed_target
        .as_ref()
        .and_then(url::Url::host_str)
        .unwrap_or(entry.target.as_str())
        .to_owned();
    let target_scheme = parsed_target
        .as_ref()
        .map(|target| target.scheme().to_ascii_uppercase());
    let target_is_ip = target_host.parse::<std::net::IpAddr>().is_ok();
    let target_badge = match (&target_scheme, target_is_ip) {
        (Some(scheme), true) => format!("{scheme} · IP TARGET"),
        (Some(scheme), false) => format!("{scheme} · HOST TARGET"),
        (None, true) => "IP TARGET".to_owned(),
        (None, false) => "HOST TARGET".to_owned(),
    };
    let scheme_behavior = target_scheme
        .as_deref()
        .map(|scheme| format!(" Use {scheme}; matching requests may omit their scheme."))
        .unwrap_or_default();
    let edit_this = this.clone();
    let edit_entry = entry.clone();
    let edit_proxy_id = proxy_id.to_owned();
    let delete_this = this.clone();
    let delete_hostname = entry.hostname.clone();
    let delete_proxy_id = proxy_id.to_owned();
    let row_selector = format!("proxy-rule-{proxy_id}-{}", entry.hostname);
    let edit_selector = format!("edit-proxy-rule-{proxy_id}-{}", entry.hostname);
    let delete_selector = format!("delete-proxy-rule-{proxy_id}-{}", entry.hostname);

    h_flex()
        .debug_selector(move || row_selector.clone())
        .w_full()
        .items_start()
        .gap_4()
        .px_5()
        .py_4()
        .border_b_1()
        .border_color(cx.api_outline_variant())
        .child(
            v_flex()
                .w(px(220.))
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_sm()
                        .font_semibold()
                        .child(entry.hostname.clone()),
                )
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(cx.theme().muted_foreground)
                        .child("Exact hostname"),
                ),
        )
        .child(
            v_flex()
                .w(px(220.))
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_sm()
                        .font_semibold()
                        .child(entry.target.clone()),
                )
                .child(request_proxy_badge(
                    target_badge,
                    if target_is_ip {
                        cx.theme().blue
                    } else {
                        cx.theme().primary
                    },
                )),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(if target_is_ip {
                    format!(
                        "Connect to {}; keep {} as HTTP Host and HTTPS SNI.{}",
                        target_host, entry.hostname, scheme_behavior
                    )
                } else {
                    format!(
                        "Connect to {}; use it as HTTP Host and HTTPS SNI.{}",
                        target_host, scheme_behavior
                    )
                }),
        )
        .child(
            h_flex()
                .w(px(152.))
                .justify_end()
                .gap_1()
                .child(
                    div().debug_selector({
                        let edit_selector = edit_selector.clone();
                        move || edit_selector.clone()
                    }).child(
                        Button::new(SharedString::from(edit_selector))
                            .icon(IconName::Settings2)
                            .label("Edit")
                            .xsmall()
                            .outline()
                            .tooltip("Edit rule")
                            .disabled(!can_update || busy)
                            .on_click(move |_, window, cx| {
                                if let Some(this) = edit_this.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.open_proxy_rule_dialog(
                                            edit_proxy_id.clone(),
                                            Some(edit_entry.clone()),
                                            window,
                                            cx,
                                        );
                                    });
                                }
                            }),
                    ),
                )
                .child(
                    div().debug_selector({
                        let delete_selector = delete_selector.clone();
                        move || delete_selector.clone()
                    }).child(
                        Button::new(SharedString::from(delete_selector))
                            .icon(IconName::Delete)
                            .label("Delete")
                            .xsmall()
                            .ghost()
                            .danger()
                            .tooltip("Delete rule")
                            .disabled(!can_update || busy)
                            .on_click(move |_, window, cx| {
                                if let Some(this) = delete_this.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.request_delete_proxy_rule(
                                            delete_proxy_id.clone(),
                                            delete_hostname.clone(),
                                            window,
                                            cx,
                                        );
                                    });
                                }
                            }),
                    ),
                ),
        )
        .into_any_element()
}

fn request_proxy_permission_note(message: impl Into<SharedString>, cx: &mut App) -> AnyElement {
    div()
        .w_full()
        .p_4()
        .rounded_lg()
        .border_1()
        .border_color(cx.api_outline_variant())
        .bg(cx.api_surface_low())
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(message.into())
        .into_any_element()
}

fn request_proxy_behavior_card(
    title: &'static str,
    description: &'static str,
    cx: &mut App,
) -> AnyElement {
    v_flex()
        .min_w_0()
        .flex_1()
        .gap_2()
        .p_4()
        .rounded_lg()
        .border_1()
        .border_color(cx.api_outline_variant())
        .bg(cx.api_surface_low())
        .child(div().text_sm().font_semibold().child(title))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(description),
        )
        .into_any_element()
}

fn request_proxy_badge(label: impl Into<SharedString>, color: Hsla) -> AnyElement {
    div()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(color.opacity(0.12))
        .text_size(px(10.))
        .font_semibold()
        .text_color(color)
        .child(label.into())
        .into_any_element()
}

fn request_proxy_empty(message: impl Into<SharedString>, cx: &mut App) -> AnyElement {
    v_flex()
        .w_full()
        .min_h(px(260.))
        .items_center()
        .justify_center()
        .gap_2()
        .rounded_lg()
        .border_1()
        .border_color(cx.api_outline_variant())
        .bg(cx.api_surface_low())
        .child(
            div()
                .text_sm()
                .font_semibold()
                .child("Request proxy unavailable"),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(message.into()),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use gpui::{TestAppContext, px, size};

    use super::*;
    use crate::core::{ManagementPermission, PROXIES_READ};

    #[gpui::test]
    fn request_proxy_renders_as_a_full_workspace_tool(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("create temporary database directory");
        let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
        store.initialize().expect("initialize test database");

        let mut app = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            let base_key_bindings = shortcuts::capture_base_key_bindings(cx);
            crate::theme::configure(cx);
            let view = cx
                .new(|cx| ApiTester::new_with_database_store(base_key_bindings, store, window, cx));
            crate::register_app_action_handlers(&view, cx);
            app = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        let app = app.expect("capture app entity");
        cx.update(|window, _| window.activate_window());
        cx.simulate_resize(size(px(1_300.), px(900.)));

        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let now = Utc::now();
                let permissions = [
                    SERVER_SETTINGS_UPDATE,
                    PROXIES_READ,
                    PROXIES_CREATE,
                    PROXIES_UPDATE,
                    PROXIES_DELETE,
                    PROXIES_ASSIGN,
                ]
                .into_iter()
                .map(|key| ManagementPermission {
                    key: key.to_owned(),
                    description: key.to_owned(),
                })
                .collect::<Vec<_>>();
                let role = ManagementRole {
                    id: "owner".to_owned(),
                    name: "Owner".to_owned(),
                    description: "Server owner".to_owned(),
                    system: true,
                    permissions,
                    created_by: None,
                    created_at: now,
                    updated_at: now,
                };
                let current_user = ManagementUser {
                    id: "user-1".to_owned(),
                    email: "owner@example.com".to_owned(),
                    display_name: "Owner".to_owned(),
                    active: true,
                    roles: vec![role],
                    created_by: None,
                    created_at: now,
                    updated_at: now,
                };
                app.settings.upstreams.upsert(UpstreamProfile {
                    id: "server-1".to_owned(),
                    base_url: "https://resolved.example.com/".to_owned(),
                    user_id: current_user.id.clone(),
                    email: current_user.email.clone(),
                    display_name: current_user.display_name.clone(),
                    session_expires_at: now + chrono::Duration::hours(1),
                    connected_at: now,
                    permission_keys: BTreeSet::from([SERVER_SETTINGS_UPDATE.to_owned()]),
                    workspaces: Vec::new(),
                    active_workspace_id: None,
                    active_environment_ids: BTreeMap::new(),
                    extra: BTreeMap::new(),
                });
                assert!(app.settings.upstreams.select("server-1"));
                app.server_management.upstream_id = Some("server-1".to_owned());
                app.server_management.status = ServerManagementStatus::Ready;
                app.server_management
                    .set_snapshot(UpstreamManagementSnapshot {
                        profiles: vec![crate::core::ProfileView {
                            id: current_user.id.clone(),
                            email: current_user.email.clone(),
                            display_name: current_user.display_name.clone(),
                            active: true,
                        }],
                        current_user,
                        users: None,
                        roles: None,
                        permissions: None,
                        workspaces: None,
                        request_execution_settings: Some(RequestExecutionSettings {
                            mode: RequestExecutionMode::Server,
                        }),
                        proxies: Some(vec![ManagementProxy {
                            id: "proxy-1".to_owned(),
                            name: "Internal routing".to_owned(),
                            rules: vec![
                                HostnameOverride {
                                    hostname: "api.internal".to_owned(),
                                    target: "10.0.0.25".to_owned(),
                                },
                                HostnameOverride {
                                    hostname: "legacy.internal".to_owned(),
                                    target: "https://gateway.internal".to_owned(),
                                },
                            ],
                            assignments: vec![crate::core::ProxyAssignment {
                                scope_kind: crate::core::ProxyScopeKind::Server,
                                scope_id: None,
                            }],
                            excluded_user_ids: Vec::new(),
                            excluded_role_ids: Vec::new(),
                            created_at: now,
                            updated_at: now,
                        }]),
                    });
                app.workspace_tabs.open_tool(WorkspaceToolTab::RequestProxy);
                cx.notify();
            });
        });
        cx.run_until_parked();

        assert!(cx.debug_bounds("request-proxy-workspace").is_some());
        assert!(cx.debug_bounds("workspace-request-proxy-tab").is_some());
        assert!(
            cx.debug_bounds("server-request-execution-enabled")
                .is_some()
        );
        assert!(cx.debug_bounds("new-proxy").is_some());
        assert!(cx.debug_bounds("request-proxy-card-proxy-1").is_some());
        assert!(
            cx.debug_bounds("proxy-rule-proxy-1-api.internal")
                .is_some()
        );
        assert!(
            cx.debug_bounds("proxy-rule-proxy-1-legacy.internal")
                .is_some()
        );
        assert!(
            cx.debug_bounds("edit-proxy-rule-proxy-1-api.internal")
                .is_some()
        );
        assert!(
            cx.debug_bounds("delete-proxy-rule-proxy-1-api.internal")
                .is_some()
        );
        assert!(
            cx.debug_bounds("manage-proxy-assignments-proxy-1")
                .is_some()
        );
        assert!(
            cx.debug_bounds("manage-proxy-exclusions-proxy-1")
                .is_some()
        );
    }
}
