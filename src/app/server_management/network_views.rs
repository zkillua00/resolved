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
            if let Some(settings) = snapshot.request_execution_settings.as_ref() {
                render_request_proxy_settings(
                    &cx.entity().downgrade(),
                    settings,
                    snapshot.has_permission(SERVER_SETTINGS_UPDATE),
                    busy,
                    cx,
                )
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
                            .child(content),
                    ),
            )
            .into_any_element()
    }
}

fn render_request_proxy_settings(
    this: &WeakEntity<ApiTester>,
    settings: &RequestExecutionSettings,
    can_update: bool,
    busy: bool,
    cx: &mut App,
) -> AnyElement {
    let server_mode = settings.mode == RequestExecutionMode::Server;
    let mode_this = this.clone();
    let mode_settings = settings.clone();
    let add_this = this.clone();

    let rows = settings
        .hostname_overrides
        .iter()
        .map(|entry| render_hostname_override_row(this, entry, can_update, busy, cx))
        .collect::<Vec<_>>();

    v_flex()
        .w_full()
        .gap_6()
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
                                            "Requests in every workspace on this server originate from the server. Host overrides below apply before it connects."
                                        } else {
                                            "Requests continue to originate from each user's computer. Host overrides stay configured but are inactive."
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
                }),
        )
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
                                        .child(
                                            div()
                                                .text_base()
                                                .font_semibold()
                                                .child("Host overrides"),
                                        )
                                        .child(request_proxy_badge(
                                            format!("{} RULES", rows.len()),
                                            cx.theme().primary,
                                        )),
                                )
                                .child(
                                    div()
                                        .max_w(px(720.))
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(
                                            "Route an exact request hostname to another hostname or IP. Prefix the target with http:// or https:// to define its scheme and allow matching request URLs to omit one.",
                                        ),
                                ),
                        )
                        .child(
                            Button::new("new-hostname-override")
                                .icon(IconName::Plus)
                                .label("Add override")
                                .small()
                                .primary()
                                .disabled(!can_update || busy)
                                .on_click(move |_, window, cx| {
                                    if let Some(this) = add_this.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.open_hostname_override_dialog(None, window, cx);
                                        });
                                    }
                                }),
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
                .child(if rows.is_empty() {
                    v_flex()
                        .w_full()
                        .min_h(px(180.))
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .p_6()
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .child("No host overrides"),
                        )
                        .child(
                            div()
                                .max_w(px(480.))
                                .text_center()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Requests use normal DNS resolution until an exact hostname rule is added."),
                        )
                        .into_any_element()
                } else {
                    v_flex().w_full().children(rows).into_any_element()
                }),
        )
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
                )),
        )
        .into_any_element()
}

fn render_hostname_override_row(
    this: &WeakEntity<ApiTester>,
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
    let delete_this = this.clone();
    let delete_hostname = entry.hostname.clone();
    let row_selector = format!("request-proxy-host-{}", entry.hostname);
    let edit_selector = format!("edit-hostname-override-{}", entry.hostname);
    let delete_selector = format!("delete-hostname-override-{}", entry.hostname);

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
                    div().debug_selector(move || edit_selector.clone()).child(
                        Button::new(SharedString::from(format!(
                            "edit-hostname-override-{}",
                            entry.hostname
                        )))
                        .icon(IconName::Settings2)
                        .label("Edit")
                        .xsmall()
                        .outline()
                        .tooltip("Edit override")
                        .disabled(!can_update || busy)
                        .on_click(move |_, window, cx| {
                            if let Some(this) = edit_this.upgrade() {
                                this.update(cx, |this, cx| {
                                    this.open_hostname_override_dialog(
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
                    div().debug_selector(move || delete_selector.clone()).child(
                        Button::new(SharedString::from(format!(
                            "delete-hostname-override-{}",
                            entry.hostname
                        )))
                        .icon(IconName::Delete)
                        .label("Delete")
                        .xsmall()
                        .ghost()
                        .danger()
                        .tooltip("Delete override")
                        .disabled(!can_update || busy)
                        .on_click(move |_, window, cx| {
                            if let Some(this) = delete_this.upgrade() {
                                this.update(cx, |this, cx| {
                                    this.request_delete_hostname_override(
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
    use crate::core::ManagementPermission;

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
                let permission = ManagementPermission {
                    key: SERVER_SETTINGS_UPDATE.to_owned(),
                    description: "Update server settings".to_owned(),
                };
                let role = ManagementRole {
                    id: "owner".to_owned(),
                    name: "Owner".to_owned(),
                    description: "Server owner".to_owned(),
                    system: true,
                    permissions: vec![permission],
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
                            hostname_overrides: vec![
                                HostnameOverride {
                                    hostname: "api.internal".to_owned(),
                                    target: "10.0.0.25".to_owned(),
                                },
                                HostnameOverride {
                                    hostname: "legacy.internal".to_owned(),
                                    target: "https://gateway.internal".to_owned(),
                                },
                            ],
                        }),
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
        assert!(cx.debug_bounds("request-proxy-host-api.internal").is_some());
        assert!(
            cx.debug_bounds("request-proxy-host-legacy.internal")
                .is_some()
        );
        assert!(
            cx.debug_bounds("edit-hostname-override-api.internal")
                .is_some()
        );
        assert!(
            cx.debug_bounds("delete-hostname-override-api.internal")
                .is_some()
        );
    }
}
