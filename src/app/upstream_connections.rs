use gpui_component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use zeroize::Zeroizing;

use super::*;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum UpstreamLoginStatus {
    #[default]
    Idle,
    Authenticating,
    SecuringSession,
    Error(String),
}

impl UpstreamLoginStatus {
    fn busy(&self) -> bool {
        matches!(self, Self::Authenticating | Self::SecuringSession)
    }

    fn label(&self) -> Option<&str> {
        match self {
            Self::Idle => None,
            Self::Authenticating => Some("Signing in…"),
            Self::SecuringSession => Some("Saving server…"),
            Self::Error(message) => Some(message),
        }
    }
}

impl ApiTester {
    fn active_workspace_tooltip(&self) -> String {
        let workspace = self.active_workspace_name();
        let attribution = self
            .workspace
            .created_by
            .as_ref()
            .map(|creator| format!(" · {}", creator_attribution(creator)))
            .unwrap_or_default();
        if !matches!(
            self.workspace_providers.active_id(),
            WorkspaceProviderId::Upstream { .. }
        ) {
            return format!("Workspace: {workspace}{attribution}");
        }
        let connection = match self.realtime_status {
            RealtimeConnectionStatus::Inactive => "",
            RealtimeConnectionStatus::Connecting => " · Connecting",
            RealtimeConnectionStatus::Connected => " · Live",
            RealtimeConnectionStatus::Reconnecting => " · Reconnecting",
            RealtimeConnectionStatus::Unavailable => " · Unavailable",
        };
        format!("Workspace: {workspace}{attribution}{connection}")
    }

    pub(super) fn upstream_settings_page(&self, cx: &mut Context<Self>) -> SettingPage {
        SettingPage::new("Servers")
            .description("Connect to self-hosted Resolved servers and switch between them.")
            .default_open(true)
            .resettable(false)
            .group(SettingGroup::new().title("Connections").items([
                self.active_upstream_setting_item(cx),
                self.connected_upstreams_setting_item(cx),
            ]))
    }

    pub(super) fn render_upstream_navigation_control(
        &self,
        item_width: Pixels,
        item_height: Pixels,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = compact_label(&self.active_workspace_name(), if compact { 10 } else { 14 });
        let tooltip = self.active_workspace_tooltip();
        let local_workspaces = self.local_workspaces.clone();
        let servers = self.settings.upstreams.servers.clone();
        let active_provider_id = self.workspace_providers.active_id().clone();
        let disabled = self.sending || self.workspace_switch_status.busy();
        let this = cx.entity().downgrade();

        Button::new("rail-workspaces")
            .debug_selector(|| "rail-workspaces".to_owned())
            .w(item_width)
            .h(item_height)
            .ghost()
            .disabled(disabled)
            .tooltip(tooltip)
            .child(
                v_flex()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .child(Icon::new(IconName::LayoutDashboard).with_size(px(18.)))
                    .when(!compact, |this| {
                        this.child(
                            div()
                                .max_w(item_width - px(8.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(10.5))
                                .font_semibold()
                                .child(label),
                        )
                    }),
            )
            .dropdown_menu(move |menu, _, _| {
                build_workspace_picker_menu(
                    menu,
                    &local_workspaces,
                    &servers,
                    &active_provider_id,
                    &this,
                )
            })
            .into_any_element()
    }

    pub(super) fn render_title_workspace_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let local_workspaces = self.local_workspaces.clone();
        let servers = self.settings.upstreams.servers.clone();
        let active_provider_id = self.workspace_providers.active_id().clone();
        let disabled = self.sending || self.workspace_switch_status.busy();
        let tooltip = self.active_workspace_tooltip();
        let this = cx.entity().downgrade();

        Button::new("title-workspaces")
            .debug_selector(|| "title-workspaces".to_owned())
            .label("Workspace")
            .dropdown_caret(true)
            .ghost()
            .compact()
            .font_semibold()
            .disabled(disabled)
            .tooltip(tooltip)
            .dropdown_menu(move |menu, _, _| {
                build_workspace_picker_menu(
                    menu,
                    &local_workspaces,
                    &servers,
                    &active_provider_id,
                    &this,
                )
            })
            .into_any_element()
    }

    pub(super) fn render_upstream_login_page(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let busy = self.upstream_login_status.busy();
        let securing = self.upstream_login_status == UpstreamLoginStatus::SecuringSession;
        let status = self.upstream_login_status.label().map(ToOwned::to_owned);
        let status_is_error = matches!(self.upstream_login_status, UpstreamLoginStatus::Error(_));
        let can_submit = self.settings_writable && !busy;

        v_flex()
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .debug_selector(|| "upstream-login-page".to_owned())
            .child(
                h_flex()
                    .h(APP_TITLE_BAR_HEIGHT)
                    .flex_shrink_0()
                    .pl(window_chrome::leading_inset())
                    .pr(window_chrome::trailing_inset())
                    .border_b_1()
                    .border_color(cx.theme().title_bar_border)
                    .bg(cx.theme().title_bar)
                    .child(
                        h_flex()
                            .gap_6()
                            .child(resolved_brand_lockup(cx))
                            .child(
                                h_flex()
                                    .h_full()
                                    .items_center()
                                    .border_b_2()
                                    .border_color(cx.theme().primary)
                                    .px_1()
                                    .text_sm()
                                    .font_semibold()
                                    .child("Connect to server"),
                            ),
                    )
                    .child(window_chrome::caption_drag_region())
                    .child(
                        h_flex()
                            .h_full()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("close-upstream-login")
                                    .icon(IconName::Close)
                                    .ghost()
                                    .rounded_full()
                                    .disabled(securing)
                                    .tooltip(if securing {
                                        "Please wait while the server is saved"
                                    } else {
                                        "Close"
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.close_upstream_login(window, cx);
                                    })),
                            )
                            .child(window_chrome::window_controls(window, cx))
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .items_center()
                    .justify_center()
                    .p_8()
                    .child(
                        v_flex()
                            .w_full()
                            .max_w(px(520.))
                            .gap_6()
                            .p_8()
                            .rounded_lg()
                            .border_1()
                            .border_color(cx.api_outline_variant())
                            .bg(cx.api_surface())
                            .shadow_lg()
                            .child(
                                v_flex()
                                    .gap_2()
                                    .child(div().text_2xl().font_semibold().child("Add a server"))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(
                                                "Resolved verifies these credentials directly with your server. The password is never stored.",
                                            ),
                                    ),
                            )
                            .child(login_field(
                                "SERVER URL",
                                Input::new(&self.upstream_login_url).large(),
                            ))
                            .child(login_field(
                                "LOGIN",
                                Input::new(&self.upstream_login_email).large(),
                            ))
                            .child(login_field(
                                "PASSWORD",
                                Input::new(&self.upstream_login_password)
                                    .large()
                                    .mask_toggle(),
                            ))
                            .when_some(status, |this, status| {
                                this.child(
                                    div()
                                        .p_3()
                                        .rounded_md()
                                        .bg(if status_is_error {
                                            cx.theme().danger.opacity(0.1)
                                        } else {
                                            cx.theme().info.opacity(0.1)
                                        })
                                        .text_sm()
                                        .text_color(if status_is_error {
                                            cx.theme().danger
                                        } else {
                                            cx.theme().info
                                        })
                                        .child(status),
                                )
                            })
                            .child(
                                v_flex().w_full().pb_6().child(
                                    Button::new("submit-upstream-login")
                                        .label(match &self.upstream_login_status {
                                            UpstreamLoginStatus::Authenticating => "Signing in…",
                                            UpstreamLoginStatus::SecuringSession => "Saving server…",
                                            UpstreamLoginStatus::Idle
                                            | UpstreamLoginStatus::Error(_) => {
                                                "Login and add server"
                                            }
                                        })
                                        .large()
                                        .primary()
                                        .w_full()
                                        .disabled(!can_submit)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.submit_upstream_login(window, cx);
                                        })),
                                ),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn open_upstream_login(
        &mut self,
        upstream_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.upstream_login_status == UpstreamLoginStatus::SecuringSession {
            return;
        }
        if let Some(abort_handle) = self.upstream_login_abort_handle.take() {
            abort_handle.abort();
        }
        self.upstream_login_generation = self.upstream_login_generation.wrapping_add(1);
        self.upstream_login_status = UpstreamLoginStatus::Idle;

        let profile = upstream_id
            .as_deref()
            .and_then(|id| self.settings.upstreams.server(id));
        let base_url = profile
            .map(|profile| profile.base_url.clone())
            .unwrap_or_default();
        let email = profile
            .map(|profile| profile.email.clone())
            .unwrap_or_default();
        self.upstream_login_url.update(cx, |input, cx| {
            input.set_value(base_url, window, cx);
        });
        self.upstream_login_email.update(cx, |input, cx| {
            input.set_value(email, window, cx);
        });
        self.upstream_login_password.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.set_masked(true, window, cx);
        });
        self.upstream_login_open = true;
        if profile.is_some() {
            self.upstream_login_password
                .read(cx)
                .focus_handle(cx)
                .focus(window);
        } else {
            self.upstream_login_url
                .read(cx)
                .focus_handle(cx)
                .focus(window);
        }
        cx.notify();
    }

    pub(super) fn close_upstream_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.upstream_login_status == UpstreamLoginStatus::SecuringSession {
            return;
        }
        if let Some(abort_handle) = self.upstream_login_abort_handle.take() {
            abort_handle.abort();
        }
        self.upstream_login_generation = self.upstream_login_generation.wrapping_add(1);
        self.upstream_login_open = false;
        self.upstream_login_status = UpstreamLoginStatus::Idle;
        self.upstream_login_password.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.set_masked(true, window, cx);
        });
        cx.notify();
    }

    pub(super) fn submit_upstream_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.upstream_login_status.busy() {
            return;
        }
        if !self.settings_writable {
            self.upstream_login_status = UpstreamLoginStatus::Error(
                "Settings are read-only, so this server cannot be added.".to_owned(),
            );
            cx.notify();
            return;
        }

        let base_url =
            match normalize_upstream_url(self.upstream_login_url.read(cx).value().as_ref()) {
                Ok(url) => url,
                Err(error) => {
                    self.upstream_login_status = UpstreamLoginStatus::Error(error.to_string());
                    cx.notify();
                    return;
                }
            };
        let login = self.upstream_login_email.read(cx).value().trim().to_owned();
        if login.is_empty() {
            self.upstream_login_status = UpstreamLoginStatus::Error("Enter your login.".to_owned());
            cx.notify();
            return;
        }
        let password = Zeroizing::new(self.upstream_login_password.read(cx).value().to_string());
        if password.is_empty() {
            self.upstream_login_status =
                UpstreamLoginStatus::Error("Enter your password.".to_owned());
            cx.notify();
            return;
        }
        // Remove the password from retained GPUI state before any await point.
        self.upstream_login_password.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });

        self.upstream_login_generation = self.upstream_login_generation.wrapping_add(1);
        let generation = self.upstream_login_generation;
        self.upstream_login_status = UpstreamLoginStatus::Authenticating;
        let client = self.upstream_client.clone();
        let task = self
            .runtime
            .spawn(async move { login_upstream(&client, base_url, login, password).await });
        self.upstream_login_abort_handle = Some(task.abort_handle());
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.upstream_login_generation != generation {
                    return;
                }
                this.upstream_login_abort_handle = None;
                match result {
                    Ok(Ok(login)) => this.secure_upstream_login(login, generation, window, cx),
                    Ok(Err(error)) => {
                        this.upstream_login_status = UpstreamLoginStatus::Error(error.to_string());
                        cx.notify();
                    }
                    Err(error) if error.is_cancelled() => {}
                    Err(error) => {
                        this.upstream_login_status =
                            UpstreamLoginStatus::Error(format!("Login failed: {error}"));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn secure_upstream_login(
        &mut self,
        login: crate::core::UpstreamLoginResult,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let crate::core::UpstreamLoginResult {
            base_url,
            token,
            expires_at,
            user,
        } = login;
        let existing_id = self
            .settings
            .upstreams
            .server_for_url(&base_url)
            .map(|profile| profile.id.clone());
        let mut profile = UpstreamProfile::from_login(existing_id, &base_url, &user, expires_at);
        if let Some(existing) = self.settings.upstreams.server(&profile.id) {
            profile.workspaces = existing.workspaces.clone();
            profile.active_workspace_id = existing.active_workspace_id.clone();
            profile.active_environment_ids = existing.active_environment_ids.clone();
        }
        let profile_label = profile.display_label();
        let upstream_id = profile.id.clone();
        let credential = UpstreamCredential::new(token, expires_at);
        let mut candidate = self.settings.clone();
        let preferred_workspace_id = profile.active_workspace_id.clone();
        candidate.upstreams.upsert(profile);
        let vault = self.credential_vault.clone();
        let persisted_candidate = candidate.clone();
        let stored_upstream_id = upstream_id.clone();
        self.upstream_login_status = UpstreamLoginStatus::SecuringSession;
        let task = self.runtime.spawn_blocking(move || {
            vault.store_upstream_with_settings(
                &persisted_candidate,
                &stored_upstream_id,
                &credential,
            )
        });
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.upstream_login_generation != generation {
                    return;
                }
                match result {
                    Ok(Ok(())) => {
                        if let Err(error) =
                            this.apply_persisted_settings(candidate.clone(), false, cx)
                        {
                            this.upstream_login_status = UpstreamLoginStatus::Error(error);
                            cx.notify();
                            return;
                        }
                        this.settings_notice = Some(format!("Connected to {profile_label}."));
                        this.upstream_login_status = UpstreamLoginStatus::Idle;
                        this.upstream_login_open = false;
                        this.upstream_login_password.update(cx, |input, cx| {
                            input.set_value("", window, cx);
                            input.set_masked(true, window, cx);
                        });
                        this.switch_to_upstream(
                            upstream_id.clone(),
                            preferred_workspace_id.clone(),
                            window,
                            cx,
                        );
                        cx.notify();
                    }
                    Ok(Err(error)) => {
                        this.upstream_login_status = UpstreamLoginStatus::Error(error.to_string());
                        cx.notify();
                    }
                    Err(error) => {
                        this.upstream_login_status = UpstreamLoginStatus::Error(format!(
                            "Could not save this server: {error}"
                        ));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn select_upstream(
        &mut self,
        upstream_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(profile) = upstream_id
            .as_deref()
            .and_then(|id| self.settings.upstreams.server(id))
            && profile.session_expired(Utc::now())
        {
            self.settings_notice = Some(format!(
                "Log in to {} again before selecting it.",
                profile.display_label()
            ));
            cx.notify();
            return;
        }
        match upstream_id {
            Some(upstream_id) => self.switch_to_upstream(upstream_id, None, window, cx),
            None => {
                let workspace_id = self
                    .database_store
                    .active_local_workspace_id()
                    .ok()
                    .or_else(|| {
                        self.local_workspaces
                            .first()
                            .map(|workspace| workspace.id.clone())
                    });
                if let Some(workspace_id) = workspace_id {
                    self.switch_to_local_workspace(workspace_id, window, cx);
                } else {
                    self.settings_notice = Some("No local workspace is available.".to_owned());
                }
            }
        }
        cx.notify();
    }

    fn open_forget_upstream_dialog(
        &mut self,
        upstream_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(profile) = self.settings.upstreams.server(&upstream_id) else {
            return;
        };
        let label = profile.display_label();
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let forget_this = this.clone();
            let forget_id = upstream_id.clone();
            dialog
                .title(format!("Forget {label}?"))
                .w(px(440.))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Forget server".to_owned())
                        .ok_variant(ButtonVariant::Danger),
                )
                .on_ok(move |_, window, cx| {
                    if let Some(this) = forget_this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.forget_upstream(&forget_id, window, cx);
                        });
                    }
                    true
                })
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "This removes the server from Resolved on this Mac. It does not delete your server account or local workspace.",
                        ),
                )
        });
    }

    fn forget_upstream(&mut self, upstream_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let active_on_server = matches!(
            self.workspace_providers.active_id(),
            WorkspaceProviderId::Upstream {
                upstream_id: active_id,
                ..
            } if active_id == upstream_id
        );
        if active_on_server {
            let Some(local_workspace_id) = self
                .database_store
                .active_local_workspace_id()
                .ok()
                .or_else(|| {
                    self.local_workspaces
                        .first()
                        .map(|workspace| workspace.id.clone())
                })
            else {
                self.settings_notice =
                    Some("No local workspace is available for switching.".to_owned());
                cx.notify();
                return;
            };
            self.switch_to_local_workspace(local_workspace_id, window, cx);
            if matches!(
                self.workspace_providers.active_id(),
                WorkspaceProviderId::Upstream {
                    upstream_id: active_id,
                    ..
                } if active_id == upstream_id
            ) {
                return;
            }
        }
        let mut candidate = self.settings.clone();
        let Some(profile) = candidate.upstreams.remove(upstream_id) else {
            return;
        };
        match self
            .credential_vault
            .delete_upstream_with_settings(&candidate, upstream_id)
        {
            Ok(()) => {
                self.workspace_providers.remove_upstream(upstream_id);
                if let Err(error) = self.apply_persisted_settings(candidate, false, cx) {
                    self.settings_notice = Some(error);
                } else {
                    self.settings_notice =
                        Some(format!("Forgot {} on this Mac.", profile.display_label()));
                }
            }
            Err(error) => {
                self.settings_notice = Some(format!("The server could not be forgotten: {error}"));
            }
        }
        cx.notify();
    }

    fn active_upstream_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let this = cx.entity().downgrade();
        SettingItem::new(
            "Active connection",
            SettingField::<SharedString>::render(move |_, _, cx| {
                let Some(entity) = this.upgrade() else {
                    return div().into_any_element();
                };
                let state = entity.read(cx);
                let selected_id = state.settings.upstreams.active_upstream_id.clone();
                let selected_label = state
                    .settings
                    .upstreams
                    .active()
                    .map(UpstreamProfile::display_label)
                    .unwrap_or_else(|| "Local".to_owned());
                let servers = state.settings.upstreams.servers.clone();
                let writable = state.settings_writable
                    && !state.upstream_login_status.busy()
                    && !state.workspace_switch_status.busy();
                let menu_this = this.clone();

                Button::new("active-upstream-picker")
                    .label(selected_label)
                    .dropdown_caret(true)
                    .outline()
                    .w(px(300.))
                    .disabled(!writable)
                    .dropdown_menu(move |menu, _, _| {
                        let local_this = menu_this.clone();
                        let mut menu = menu.min_w(px(300.)).item(
                            PopupMenuItem::new("Local")
                                .checked(selected_id.is_none())
                                .on_click(move |_, window, cx| {
                                    if let Some(this) = local_this.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.select_upstream(None, window, cx);
                                        });
                                    }
                                }),
                        );
                        for server in &servers {
                            let server_this = menu_this.clone();
                            let server_id = server.id.clone();
                            let checked = selected_id.as_deref() == Some(server.id.as_str());
                            let expired = server.session_expired(Utc::now());
                            menu = menu.item(
                                PopupMenuItem::new(if expired {
                                    format!("{} (login required)", server.display_label())
                                } else {
                                    server.display_label()
                                })
                                .checked(checked)
                                .disabled(expired)
                                .on_click(move |_, window, cx| {
                                    if let Some(this) = server_this.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.select_upstream(
                                                Some(server_id.clone()),
                                                window,
                                                cx,
                                            );
                                        });
                                    }
                                }),
                            );
                        }
                        let add_this = menu_this.clone();
                        menu.separator()
                            .item(PopupMenuItem::new("Add server…").on_click(
                                move |_, window, cx| {
                                    if let Some(this) = add_this.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.open_upstream_login(None, window, cx);
                                        });
                                    }
                                },
                            ))
                    })
                    .into_any_element()
            }),
        )
        .description("Choose Local or a connected server.")
    }

    fn connected_upstreams_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let this = cx.entity().downgrade();
        let mut search_text = "servers upstream login local switch connection".to_owned();
        for server in &self.settings.upstreams.servers {
            search_text.push(' ');
            search_text.push_str(&server.base_url);
            search_text.push(' ');
            search_text.push_str(&server.email);
        }

        SettingItem::render_searchable(search_text, move |_, _, cx| {
            let Some(entity) = this.upgrade() else {
                return div().into_any_element();
            };
            let state = entity.read(cx);
            let servers = state.settings.upstreams.servers.clone();
            let active_id = state.settings.upstreams.active_upstream_id.clone();
            let writable = state.settings_writable
                && !state.upstream_login_status.busy()
                && !state.workspace_switch_status.busy();
            let mut rows = Vec::with_capacity(servers.len() + 1);

            for server in servers {
                let relogin_this = this.clone();
                let forget_this = this.clone();
                let relogin_id = server.id.clone();
                let forget_id = server.id.clone();
                let active = active_id.as_deref() == Some(server.id.as_str());
                let expired = server.session_expired(Utc::now());
                let expires = server
                    .session_expires_at
                    .with_timezone(&Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string();
                rows.push(
                    h_flex()
                        .id(SharedString::from(format!("upstream-row-{}", server.id)))
                        .w_full()
                        .min_h(px(64.))
                        .gap_3()
                        .px_3()
                        .py_2()
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
                                        .child(
                                            div()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_sm()
                                                .font_semibold()
                                                .child(server.display_label()),
                                        )
                                        .when(active && !expired, |this| {
                                            let (label, color) = match state.realtime_status {
                                                RealtimeConnectionStatus::Inactive => {
                                                    ("Selected", cx.theme().info)
                                                }
                                                RealtimeConnectionStatus::Connecting => {
                                                    ("Connecting", cx.theme().info)
                                                }
                                                RealtimeConnectionStatus::Connected => {
                                                    ("Live", cx.theme().success)
                                                }
                                                RealtimeConnectionStatus::Reconnecting => {
                                                    ("Reconnecting", cx.theme().warning)
                                                }
                                                RealtimeConnectionStatus::Unavailable => {
                                                    ("Unavailable", cx.theme().danger)
                                                }
                                            };
                                            this.child(connection_badge(label, color))
                                        })
                                        .when(expired, |this| {
                                            this.child(connection_badge(
                                                "Login required",
                                                cx.theme().warning,
                                            ))
                                        }),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!(
                                            "{} · {} · session expires {expires}",
                                            server.display_name, server.email
                                        )),
                                ),
                        )
                        .child(
                            h_flex()
                                .flex_shrink_0()
                                .gap_1()
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "relogin-upstream-{}",
                                        server.id
                                    )))
                                    .label("Log in again")
                                    .small()
                                    .outline()
                                    .disabled(!writable)
                                    .on_click(
                                        move |_, window, cx| {
                                            if let Some(this) = relogin_this.upgrade() {
                                                this.update(cx, |this, cx| {
                                                    this.open_upstream_login(
                                                        Some(relogin_id.clone()),
                                                        window,
                                                        cx,
                                                    );
                                                });
                                            }
                                        },
                                    ),
                                )
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "forget-upstream-{}",
                                        server.id
                                    )))
                                    .label("Forget")
                                    .small()
                                    .ghost()
                                    .disabled(!writable)
                                    .on_click(
                                        move |_, window, cx| {
                                            if let Some(this) = forget_this.upgrade() {
                                                this.update(cx, |this, cx| {
                                                    this.open_forget_upstream_dialog(
                                                        forget_id.clone(),
                                                        window,
                                                        cx,
                                                    );
                                                });
                                            }
                                        },
                                    ),
                                ),
                        )
                        .into_any_element(),
                );
            }

            let add_this = this.clone();
            let has_connected_servers = !rows.is_empty();
            rows.push(
                h_flex()
                    .w_full()
                    .justify_between()
                    .gap_3()
                    .pt_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(if has_connected_servers {
                                "Manage your connected servers."
                            } else {
                                "No servers have been added."
                            }),
                    )
                    .child(
                        Button::new("add-upstream-server")
                            .icon(IconName::Plus)
                            .label("Add server")
                            .small()
                            .primary()
                            .disabled(!writable)
                            .on_click(move |_, window, cx| {
                                if let Some(this) = add_this.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.open_upstream_login(None, window, cx);
                                    });
                                }
                            }),
                    )
                    .into_any_element(),
            );

            v_flex()
                .w_full()
                .min_w(px(480.))
                .debug_selector(|| "upstream-settings-list".to_owned())
                .children(rows)
                .into_any_element()
        })
    }
}

fn build_workspace_picker_menu(
    mut menu: PopupMenu,
    local_workspaces: &[LocalWorkspace],
    servers: &[UpstreamProfile],
    active_provider_id: &WorkspaceProviderId,
    this: &WeakEntity<ApiTester>,
) -> PopupMenu {
    menu = menu
        .min_w(px(280.))
        .item(PopupMenuItem::new("Local").disabled(true));
    for workspace in local_workspaces {
        let workspace_this = this.clone();
        let workspace_id = workspace.id.clone();
        let checked = active_provider_id == &WorkspaceProviderId::Local(workspace.id.clone());
        menu = menu.item(
            PopupMenuItem::new(workspace.name.clone())
                .checked(checked)
                .on_click(move |_, window, cx| {
                    if let Some(this) = workspace_this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.switch_to_local_workspace(workspace_id.clone(), window, cx);
                        });
                    }
                }),
        );
    }

    let create_this = this.clone();
    menu = menu.item(
        PopupMenuItem::new("New workspace…").on_click(move |_, window, cx| {
            if let Some(this) = create_this.upgrade() {
                this.update(cx, |this, cx| {
                    this.open_create_local_workspace_dialog(window, cx);
                });
            }
        }),
    );

    for server in servers {
        menu = menu
            .separator()
            .item(PopupMenuItem::new(server.display_label()).disabled(true));
        if server.workspaces.is_empty() {
            let server_this = this.clone();
            let server_id = server.id.clone();
            menu = menu.item(
                PopupMenuItem::new("Load workspaces…")
                    .disabled(server.session_expired(Utc::now()))
                    .on_click(move |_, window, cx| {
                        if let Some(this) = server_this.upgrade() {
                            this.update(cx, |this, cx| {
                                this.switch_to_upstream(server_id.clone(), None, window, cx);
                            });
                        }
                    }),
            );
        } else {
            for workspace in &server.workspaces {
                let workspace_this = this.clone();
                let server_id = server.id.clone();
                let workspace_id = workspace.id.clone();
                let checked = active_provider_id
                    == &WorkspaceProviderId::Upstream {
                        upstream_id: server.id.clone(),
                        workspace_id: workspace.id.clone(),
                    };
                menu = menu.item(
                    PopupMenuItem::new(workspace.name.clone())
                        .checked(checked)
                        .disabled(server.session_expired(Utc::now()))
                        .on_click(move |_, window, cx| {
                            if let Some(this) = workspace_this.upgrade() {
                                this.update(cx, |this, cx| {
                                    this.switch_to_upstream(
                                        server_id.clone(),
                                        Some(workspace_id.clone()),
                                        window,
                                        cx,
                                    );
                                });
                            }
                        }),
                );
            }
        }

        let create_this = this.clone();
        let create_server_id = server.id.clone();
        menu = menu.item(
            PopupMenuItem::new("New workspace…")
                .disabled(
                    server.session_expired(Utc::now()) || !server.has_permission(WORKSPACES_CREATE),
                )
                .on_click(move |_, window, cx| {
                    if let Some(this) = create_this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.open_create_upstream_workspace_dialog(
                                create_server_id.clone(),
                                window,
                                cx,
                            );
                        });
                    }
                }),
        );

        let active_workspace_id = match active_provider_id {
            WorkspaceProviderId::Upstream {
                upstream_id,
                workspace_id,
            } if upstream_id == &server.id => Some(workspace_id.clone()),
            _ => None,
        };
        if let Some(workspace_id) = active_workspace_id {
            let rename_this = this.clone();
            let rename_server_id = server.id.clone();
            let rename_workspace_id = workspace_id.clone();
            let delete_this = this.clone();
            let delete_server_id = server.id.clone();
            let delete_workspace_id = workspace_id;
            menu = menu
                .item(
                    PopupMenuItem::new("Rename current workspace…")
                        .disabled(
                            server.session_expired(Utc::now())
                                || !server.has_permission(WORKSPACES_UPDATE),
                        )
                        .on_click(move |_, window, cx| {
                            if let Some(this) = rename_this.upgrade() {
                                this.update(cx, |this, cx| {
                                    this.open_rename_upstream_workspace_dialog(
                                        rename_server_id.clone(),
                                        rename_workspace_id.clone(),
                                        window,
                                        cx,
                                    );
                                });
                            }
                        }),
                )
                .item(
                    PopupMenuItem::new("Delete current workspace…")
                        .disabled(
                            server.session_expired(Utc::now())
                                || !server.has_permission(WORKSPACES_DELETE),
                        )
                        .on_click(move |_, window, cx| {
                            if let Some(this) = delete_this.upgrade() {
                                this.update(cx, |this, cx| {
                                    this.open_delete_upstream_workspace_dialog(
                                        delete_server_id.clone(),
                                        delete_workspace_id.clone(),
                                        window,
                                        cx,
                                    );
                                });
                            }
                        }),
                );
        }
    }

    let add_server_this = this.clone();
    menu.separator().item(
        PopupMenuItem::new("Add server…").on_click(move |_, window, cx| {
            if let Some(this) = add_server_this.upgrade() {
                this.update(cx, |this, cx| {
                    this.open_upstream_login(None, window, cx);
                });
            }
        }),
    )
}

fn login_field(label: &'static str, input: Input) -> AnyElement {
    v_flex()
        .gap_2()
        .child(div().text_xs().font_semibold().child(label))
        .child(input)
        .into_any_element()
}

fn connection_badge(label: &'static str, color: Hsla) -> AnyElement {
    div()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(color.opacity(0.12))
        .text_xs()
        .font_semibold()
        .text_color(color)
        .child(label)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, VisualTestContext, px, size};

    use super::*;

    fn mount_app(
        cx: &mut TestAppContext,
    ) -> (Entity<ApiTester>, &mut VisualTestContext, tempfile::TempDir) {
        let directory = tempfile::tempdir().expect("create temporary database directory");
        let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
        store.initialize().expect("initialize test database");

        let mut app = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            let base_key_bindings = shortcuts::capture_base_key_bindings(cx);
            crate::theme::configure(cx);
            let view = cx
                .new(|cx| ApiTester::new_with_database_store(base_key_bindings, store, window, cx));
            crate::register_app_action_handlers(&view, cx);
            app = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        visual.update(|window, _| window.activate_window());
        visual.simulate_resize(size(px(1_200.), px(800.)));
        (app.expect("capture app entity"), visual, directory)
    }

    #[gpui::test]
    fn title_bar_workspace_picker_mounts(cx: &mut TestAppContext) {
        let (_app, cx, _directory) = mount_app(cx);
        cx.run_until_parked();

        assert!(cx.debug_bounds("title-workspaces").is_some());
    }

    #[gpui::test]
    fn login_page_closing_clears_the_password(cx: &mut TestAppContext) {
        let (app, cx, _directory) = mount_app(cx);
        cx.run_until_parked();

        assert!(cx.debug_bounds("rail-workspaces").is_some());
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_upstream_login(None, window, cx);
            });
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("upstream-login-page").is_some());

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.upstream_login_password.update(cx, |input, cx| {
                    input.set_value("never-persist-me", window, cx);
                });
                app.close_upstream_login(window, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let app = app.read(cx);
            assert!(!app.upstream_login_open);
            assert!(app.upstream_login_password.read(cx).value().is_empty());
        });
    }

    #[gpui::test]
    fn servers_is_the_default_settings_page(cx: &mut TestAppContext) {
        let (app, cx, _directory) = mount_app(cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_workspace_tool_tab(WorkspaceToolTab::Settings, window, cx);
            });
        });
        cx.run_until_parked();

        assert!(
            cx.debug_bounds("upstream-settings-list").is_some(),
            "Settings must open directly to the switchable Servers page"
        );
    }

    #[gpui::test]
    fn server_tools_only_exist_while_an_upstream_workspace_is_active(cx: &mut TestAppContext) {
        let (app, cx, _directory) = mount_app(cx);
        cx.run_until_parked();
        assert!(cx.debug_bounds("rail-server-tools").is_none());

        let local_workspace_id = cx.update(|_, cx| {
            let app = app.read(cx);
            let WorkspaceProviderId::Local(workspace_id) = app.workspace_providers.active_id()
            else {
                panic!("the test app must start on a local workspace")
            };
            workspace_id.clone()
        });
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let workspace = Workspace::default();
                let request_tabs = RequestTabs::default();
                let provider = RemoteWorkspaceProvider::new(
                    app.database_store.clone(),
                    "server-1".to_owned(),
                    "workspace-1".to_owned(),
                    workspace.clone(),
                );
                let provider_id = provider.id();
                app.workspace_providers.register(Arc::new(provider));
                app.activate_loaded_workspace(
                    provider_id,
                    workspace,
                    request_tabs,
                    false,
                    true,
                    window,
                    cx,
                );
                app.workspace_tabs.open_tool(WorkspaceToolTab::ServerTools);
                cx.notify();
            });
        });
        cx.run_until_parked();

        assert!(cx.debug_bounds("rail-server-tools").is_some());
        assert!(cx.debug_bounds("workspace-server-tools-tab").is_some());
        assert!(cx.debug_bounds("server-tools-workspace").is_some());

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.switch_to_local_workspace(local_workspace_id.clone(), window, cx);
            });
        });
        cx.run_until_parked();

        let (active_provider, notice) = cx.update(|_, cx| {
            let app = app.read(cx);
            (
                app.workspace_providers.active_id().clone(),
                app.settings_notice.clone(),
            )
        });
        assert_eq!(
            active_provider,
            WorkspaceProviderId::Local(local_workspace_id),
            "local switch failed: {notice:?}"
        );
        assert!(cx.update(|_, cx| !app.read(cx).workspace_tabs.server_tools_open()));
    }

    #[gpui::test]
    fn local_workspace_switch_replaces_the_active_workspace(cx: &mut TestAppContext) {
        let (app, cx, _directory) = mount_app(cx);
        let (default_id, second_id) = cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let default_id = app.database_store.active_local_workspace_id().unwrap();
                let mut default_workspace = app.workspace.clone();
                default_workspace
                    .create_collection("Default collection")
                    .unwrap();
                app.commit_workspace(default_workspace).unwrap();

                let second = app.database_store.create_local_workspace("Second").unwrap();
                let mut second_workspace = Workspace::default();
                second_workspace
                    .create_collection("Second collection")
                    .unwrap();
                app.database_store
                    .save_workspace_for(&second.id, &second_workspace)
                    .unwrap();
                app.local_workspaces.push(second.clone());
                app.switch_to_local_workspace(second.id.clone(), window, cx);
                (default_id, second.id)
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let app = app.read(cx);
            assert_eq!(app.active_workspace_name(), "Second");
            assert_eq!(app.workspace.collections[0].name, "Second collection");
        });

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.url.update(cx, |url, cx| {
                    url.set_value("https://buffered.example.test", window, cx)
                });
            });
        });

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.switch_to_local_workspace(default_id.clone(), window, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let app = app.read(cx);
            assert_eq!(
                app.workspace_providers.active_id(),
                &WorkspaceProviderId::Local(default_id)
            );
            assert_eq!(app.workspace.collections[0].name, "Default collection");
            assert_ne!(
                second_id,
                app.database_store.active_local_workspace_id().unwrap()
            );
        });

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.switch_to_local_workspace(second_id, window, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let app = app.read(cx);
            assert_eq!(app.url.read(cx).value(), "https://buffered.example.test");
        });
    }

    #[gpui::test]
    fn local_workspace_creation_dialog_mounts(cx: &mut TestAppContext) {
        let (app, cx, _directory) = mount_app(cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_create_local_workspace_dialog(window, cx);
            });
        });
        cx.run_until_parked();

        assert!(cx.debug_bounds("local-workspace-create-dialog").is_some());
    }

    #[gpui::test]
    fn server_workspace_creation_dialog_mounts(cx: &mut TestAppContext) {
        let (app, cx, _directory) = mount_app(cx);
        let base_url = normalize_upstream_url("https://resolved.example.com").unwrap();
        let mut profile = UpstreamProfile::from_login(
            None,
            &base_url,
            &crate::core::LoginUser {
                id: "user-1".to_owned(),
                email: "owner".to_owned(),
                display_name: "Owner".to_owned(),
                active: true,
                roles: Vec::new(),
            },
            Utc::now() + chrono::Duration::hours(1),
        );
        profile.permission_keys.insert(WORKSPACES_CREATE.to_owned());
        let upstream_id = profile.id.clone();
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.settings.upstreams.upsert(profile.clone());
                app.open_create_upstream_workspace_dialog(upstream_id.clone(), window, cx);
            });
        });
        cx.run_until_parked();

        assert!(
            cx.debug_bounds("upstream-workspace-create-dialog")
                .is_some()
        );
    }
}
