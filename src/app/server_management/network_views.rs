use super::*;

pub(super) fn render_request_execution_management(
    this: &WeakEntity<ApiTester>,
    cx: &mut App,
) -> AnyElement {
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
        return management_empty("Request execution settings are unavailable.", cx);
    };
    let Some(settings) = snapshot.request_execution_settings.as_ref() else {
        return management_empty(
            "You do not have permission to view server request execution settings.",
            cx,
        );
    };

    let busy = management.status.busy();
    let can_update = snapshot.has_permission(SERVER_SETTINGS_UPDATE);
    let server_mode = settings.mode == RequestExecutionMode::Server;
    let mode_this = this.clone();
    let mode_settings = settings.clone();
    let add_this = this.clone();
    let refresh_this = this.clone();

    let rows = settings
        .hostname_overrides
        .iter()
        .map(|entry| {
            let target_is_ip = entry.target.parse::<std::net::IpAddr>().is_ok();
            let edit_this = this.clone();
            let edit_entry = entry.clone();
            let delete_this = this.clone();
            let delete_hostname = entry.hostname.clone();
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
                                .child(entry.hostname.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(if target_is_ip {
                                    format!(
                                        "Connect to {}; keep {} as the HTTP and TLS host",
                                        entry.target, entry.hostname
                                    )
                                } else {
                                    format!(
                                        "Connect to {}; use it as the HTTP and TLS host",
                                        entry.target
                                    )
                                }),
                        ),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new(SharedString::from(format!(
                                "edit-hostname-override-{}",
                                entry.hostname
                            )))
                            .label("Edit")
                            .xsmall()
                            .outline()
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
                        )
                        .child(
                            Button::new(SharedString::from(format!(
                                "delete-hostname-override-{}",
                                entry.hostname
                            )))
                            .icon(IconName::Delete)
                            .xsmall()
                            .ghost()
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
                )
                .into_any_element()
        })
        .collect::<Vec<_>>();

    v_flex()
        .id("request-execution-management")
        .size_full()
        .min_h_0()
        .overflow_y_scroll()
        .p_4()
        .gap_5()
        .child(
            v_flex()
                .gap_2()
                .child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .child(discord_views::management_section_title(
                            "EXECUTION LOCATION",
                        ))
                        .child(
                            Button::new("refresh-request-execution-settings")
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
                    h_flex()
                        .w_full()
                        .gap_3()
                        .py_3()
                        .border_y_1()
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
                                        .child("Run requests from this server"),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if server_mode {
                                            "Enabled: server-workspace requests originate from this server."
                                        } else {
                                            "Disabled: server-workspace requests originate from each user's computer."
                                        }),
                                ),
                        )
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
                ),
        )
        .child(
            v_flex()
                .gap_2()
                .child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .gap_3()
                        .child(
                            v_flex()
                                .gap_1()
                                .child(discord_views::management_section_title("HOSTNAME OVERRIDES"))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(
                                            "IP targets behave like DNS and preserve the requested host. Hostname targets become the outgoing HTTP and TLS host.",
                                        ),
                                ),
                        )
                        .child(
                            Button::new("new-hostname-override")
                                .icon(IconName::Plus)
                                .label("Add override")
                                .small()
                                .outline()
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
                .child(if rows.is_empty() {
                    discord_views::management_detail_empty(
                        "No hostname overrides are configured.",
                        cx,
                    )
                } else {
                    v_flex()
                        .w_full()
                        .border_t_1()
                        .border_color(cx.api_outline_variant())
                        .children(rows)
                        .into_any_element()
                }),
        )
        .into_any_element()
}
