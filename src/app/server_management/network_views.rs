use super::*;
use gpui_component::Icon;

use super::proxy_rule_editor::parse_proxy_rule_target;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ProxyTab {
    #[default]
    Rules,
    Assignments,
    Exclusions,
}

/// This view belongs to the server, not the currently open request workspace.
/// Inputs and selection survive same-server refreshes, but never a server switch.
#[derive(Clone, Default)]
pub(super) struct RequestProxyState {
    pub(super) selected_proxy_id: Option<String>,
    pub(super) selected_rule_hostname: Option<String>,
    tab: ProxyTab,
    show_limits: bool,
    proxy_search: Option<Entity<InputState>>,
    rule_search: Option<Entity<InputState>>,
    subscriptions: Rc<Vec<gpui::Subscription>>,
}

impl std::fmt::Debug for RequestProxyState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestProxyState")
            .field("selected_proxy_id", &self.selected_proxy_id)
            .field("selected_rule_hostname", &self.selected_rule_hostname)
            .field("tab", &self.tab)
            .field("show_limits", &self.show_limits)
            .finish_non_exhaustive()
    }
}

impl RequestProxyState {
    pub(super) fn select_proxy(&mut self, proxy: &ManagementProxy) {
        self.selected_proxy_id = Some(proxy.id.clone());
        self.selected_rule_hostname = proxy.rules.first().map(|rule| rule.hostname.clone());
        self.tab = ProxyTab::Rules;
        self.show_limits = false;
    }

    pub(super) fn select_proxy_by_id(&mut self, id: &str, proxies: Option<&[ManagementProxy]>) {
        if let Some(proxy) = proxies
            .unwrap_or_default()
            .iter()
            .find(|proxy| proxy.id == id)
        {
            self.select_proxy(proxy);
        }
    }

    fn selected_proxy<'a>(&self, proxies: &'a [ManagementProxy]) -> Option<&'a ManagementProxy> {
        proxies
            .iter()
            .find(|proxy| Some(&proxy.id) == self.selected_proxy_id.as_ref())
    }

    fn selected_rule<'a>(&self, proxy: &'a ManagementProxy) -> Option<&'a HostnameOverride> {
        proxy
            .rules
            .iter()
            .find(|rule| Some(&rule.hostname) == self.selected_rule_hostname.as_ref())
    }

    pub(super) fn reconcile(&mut self, snapshot: &UpstreamManagementSnapshot) {
        let proxies = snapshot.proxies.as_deref().unwrap_or_default();
        if self.selected_proxy(proxies).is_none() {
            if let Some(proxy) = proxies.first() {
                self.select_proxy(proxy);
            } else {
                self.selected_proxy_id = None;
                self.selected_rule_hostname = None;
            }
        }
        if let Some(proxy) = self.selected_proxy(proxies)
            && self.selected_rule(proxy).is_none()
        {
            self.selected_rule_hostname = proxy.rules.first().map(|rule| rule.hostname.clone());
        }
    }
}

fn filter_matches(query: &str, values: &[&str]) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty()
        || values
            .iter()
            .any(|value| value.to_lowercase().contains(&query))
}

fn proxy_scope_summary(proxy: &ManagementProxy) -> String {
    match proxy.assignments.as_slice() {
        [] => "Unassigned".to_owned(),
        [assignment] => match assignment.scope_kind {
            ProxyScopeKind::Server => "Server-wide".to_owned(),
            ProxyScopeKind::Workspace => "1 workspace".to_owned(),
            ProxyScopeKind::Collection => "1 collection".to_owned(),
            ProxyScopeKind::Request => "1 saved request".to_owned(),
        },
        assignments => format!("{} scopes", assignments.len()),
    }
}

fn count_label(count: usize, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

impl ApiTester {
    pub(super) fn ensure_proxy_workspace_inputs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .server_management
            .proxy_workspace
            .proxy_search
            .is_some()
        {
            return;
        }
        let proxy_search = cx.new(|cx| InputState::new(window, cx).placeholder("Find a proxy…"));
        let rule_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter hostname or target…"));
        let subscriptions = vec![
            cx.observe(&proxy_search, |_, _, cx| cx.notify()),
            cx.observe(&rule_search, |_, _, cx| cx.notify()),
        ];
        let state = &mut self.server_management.proxy_workspace;
        state.proxy_search = Some(proxy_search);
        state.rule_search = Some(rule_search);
        state.subscriptions = Rc::new(subscriptions);
    }

    fn proxy_navigation_locked(&self) -> bool {
        self.server_management.status.busy()
            || self.server_management.proxy_rule_editor.is_editing()
            || self.server_management.proxy_scope_editor.is_editing()
    }

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
                        .child("Request proxies"),
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
        let server_label = self
            .settings
            .upstreams
            .active()
            .map(UpstreamProfile::display_label)
            .unwrap_or_else(|| "No server selected".to_owned());
        let status = management_status_element(
            &management.status,
            management.upstream_id.as_deref(),
            management.snapshot.is_some(),
            self.settings.upstreams.active_upstream_id.as_deref(),
            cx,
        );
        let content = if let Some(status) = status {
            div()
                .flex()
                .flex_row()
                .flex_1()
                .min_h_0()
                .child(div().flex_1().min_w_0().p_6().child(status))
                .children(self.render_active_proxy_rule_editor(cx))
                .when(
                    matches!(management.status, ServerManagementStatus::Error(_)),
                    |body| body.children(self.render_orphaned_proxy_scope_editor(cx)),
                )
                .into_any_element()
        } else if let Some(snapshot) = management.snapshot.as_ref() {
            div().flex().flex_row().flex_1().min_h_0().w_full()
                .child(self.render_proxy_sidebar(snapshot, cx))
                .child(if management.proxy_workspace.show_limits {
                    v_flex().id("request-proxy-limits").flex_1().min_w_0().min_h_0()
                        .overflow_y_scroll().p_6().child(self.render_execution_limits(cx)).into_any_element()
                } else if let Some(proxy) = snapshot.proxies.as_deref()
                    .and_then(|proxies| management.proxy_workspace.selected_proxy(proxies))
                {
                    self.render_proxy_detail(proxy, snapshot, cx)
                } else if let Some(editor) = self.render_orphaned_proxy_scope_editor(cx) {
                    editor
                } else {
                    let (title, message) = match snapshot.proxies.as_deref() {
                        Some([]) => ("No proxies yet", "Create a proxy, add host overrides, then assign it to a scope. Until then, requests use normal DNS."),
                        Some(_) => ("Select a proxy", "The previously selected proxy is no longer available. Choose another proxy from the list."),
                        None => ("Proxy settings unavailable", "Viewing proxies requires proxies.read. Execution location and execution limits have separate permissions."),
                    };
                    div().flex().flex_row().flex_1().min_w_0().min_h_0()
                        .child(proxy_empty(title, message, cx))
                        .children(self.render_active_proxy_rule_editor(cx))
                        .into_any_element()
                })
                .into_any_element()
        } else {
            proxy_empty(
                "Proxy settings unavailable",
                "Refresh to load this server's settings.",
                cx,
            )
        };

        v_flex()
            .id("request-proxy-workspace")
            .debug_selector(|| "request-proxy-workspace".to_owned())
            .size_full()
            .min_h_0()
            .overflow_hidden()
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
                h_flex()
                    .w_full()
                    .flex_shrink_0()
                    .px_6()
                    .py_5()
                    .gap_5()
                    .border_b_1()
                    .border_color(cx.api_outline_variant())
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(22.))
                                    .font_semibold()
                                    .child("Request proxies"),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(Icon::new(IconName::Globe).with_size(px(13.)))
                                    .child(div().truncate().child(server_label)),
                            ),
                    )
                    .child(self.render_proxy_execution_control(cx))
                    .child(
                        Button::new("refresh-request-proxy-settings")
                            .icon(IconName::Redo2)
                            .small()
                            .ghost()
                            .tooltip("Refresh proxy settings")
                            .disabled(
                                management.status.busy()
                                    || self.settings.upstreams.active().is_none(),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.refresh_server_management(window, cx)
                            })),
                    ),
            )
            .child(content)
            .into_any_element()
    }

    fn render_proxy_execution_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let snapshot = self.server_management.snapshot.as_ref();
        let settings = snapshot.and_then(|snapshot| snapshot.request_execution_settings.as_ref());
        let mode = settings.map(|settings| settings.mode);
        let can_update =
            snapshot.is_some_and(|snapshot| snapshot.has_permission(SERVER_SETTINGS_UPDATE));
        let disabled = !can_update || mode.is_none() || self.proxy_navigation_locked();
        let this = cx.entity().downgrade();
        h_flex().gap_3()
            .child(v_flex().items_end()
                .child(div().text_xs().child("Execution location"))
                .child(div().text_size(px(10.)).text_color(cx.theme().muted_foreground).child("All server workspaces")))
            .child(div().debug_selector(|| "proxy-execution-location".to_owned()).child(
                Button::new("proxy-execution-location")
                    .label(match mode {
                        Some(RequestExecutionMode::Server) => "Server",
                        Some(RequestExecutionMode::Local) => "Local",
                        None => "Unavailable",
                    })
                    .small().outline().disabled(disabled)
                    .tooltip(if settings.is_none() {
                        "Viewing execution location requires server_settings.read."
                    } else if !can_update {
                        "Read-only: changing execution location requires server_settings.update."
                    } else if self.proxy_navigation_locked() {
                        "Finish the current edit before changing execution location."
                    } else {
                        "Choose where HTTP requests and WebSockets run for all server workspaces."
                    })
                    .dropdown_menu(move |menu, _, _| {
                        [
                            (RequestExecutionMode::Server, "Server — apply assigned proxies"),
                            (RequestExecutionMode::Local, "Local — each user's device"),
                        ].into_iter().fold(menu, |menu, (next, label)| {
                            let this = this.clone();
                            menu.item(PopupMenuItem::new(label).checked(mode == Some(next))
                                .disabled(disabled || mode == Some(next))
                                .on_click(move |_, window, cx| {
                                    if let Some(this) = this.upgrade() {
                                        this.update(cx, |this, cx| this.confirm_proxy_execution_mode(next, window, cx));
                                    }
                                }))
                        })
                    }),
            ))
            .into_any_element()
    }

    fn confirm_proxy_execution_mode(
        &mut self,
        mode: RequestExecutionMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.proxy_navigation_locked() {
            return;
        }
        let upstream_id = self.server_management.upstream_id.clone();
        let server = self
            .settings
            .upstreams
            .active()
            .map(UpstreamProfile::display_label)
            .unwrap_or_default();
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let this = this.clone();
            let upstream_id = upstream_id.clone();
            dialog.title("Change execution location?").w(px(480.)).confirm()
                .button_props(DialogButtonProps::default().ok_text(match mode {
                    RequestExecutionMode::Server => "Run on server",
                    RequestExecutionMode::Local => "Run locally",
                }))
                .child(div().text_sm().text_color(cx.theme().muted_foreground).child(format!(
                    "This affects HTTP requests and WebSockets in every workspace on {server}. {} Local workspaces are unaffected.",
                    match mode {
                        RequestExecutionMode::Server => "Subsequent requests run on the server and apply assigned proxy rules.",
                        RequestExecutionMode::Local => "Subsequent requests run on each user's device. Proxy rules stay configured but are not applied.",
                    },
                )))
                .on_ok(move |_, window, cx| {
                    if let Some(this) = this.upgrade() {
                        this.update(cx, |this, cx| {
                            if this.server_management.upstream_id != upstream_id
                                || this.settings.upstreams.active_upstream_id != upstream_id
                                || this.proxy_navigation_locked()
                            {
                                return;
                            }
                            let Some(snapshot) = this.server_management.snapshot.as_ref()
                                .filter(|snapshot| snapshot.has_permission(SERVER_SETTINGS_UPDATE))
                            else { return; };
                            if let Some(mut settings) = snapshot.request_execution_settings.clone() {
                                settings.mode = mode;
                                this.run_management_mutation(
                                    ManagementMutation::UpdateRequestExecutionSettings { settings }, window, cx,
                                );
                            }
                        });
                    }
                    true
                })
        });
    }

    fn render_proxy_sidebar(
        &self,
        snapshot: &UpstreamManagementSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = &self.server_management.proxy_workspace;
        let proxies = snapshot.proxies.as_deref().unwrap_or_default();
        let query = state
            .proxy_search
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default();
        let locked = self.proxy_navigation_locked();
        let can_create = snapshot.has_permission(PROXIES_CREATE);
        let mut rows = Vec::new();
        for proxy in proxies
            .iter()
            .filter(|proxy| filter_matches(&query, &[&proxy.name]))
        {
            let id = proxy.id.clone();
            let selected = !state.show_limits && state.selected_proxy_id.as_deref() == Some(&id);
            let selector = format!("proxy-list-{}", proxy.id);
            rows.push(
                v_flex()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector.clone())
                    .w_full()
                    .px_3()
                    .py_3()
                    .gap_1()
                    .rounded_md()
                    .when(selected, |row| row.bg(cx.theme().sidebar_accent))
                    .when(!locked, |row| {
                        row.cursor_pointer()
                            .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.62)))
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Icon::new(IconName::Globe)
                                    .with_size(px(14.))
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .font_semibold()
                                    .child(proxy.name.clone()),
                            ),
                    )
                    .child(
                        div()
                            .pl_5()
                            .text_size(px(10.))
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{} · {}",
                                proxy_scope_summary(proxy),
                                count_label(proxy.rules.len(), "rule")
                            )),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.proxy_navigation_locked() {
                            return;
                        }
                        if let Some(proxy) = this.management_proxy(&id).cloned() {
                            this.server_management.proxy_workspace.select_proxy(&proxy);
                            if let Some(search) =
                                &this.server_management.proxy_workspace.rule_search
                            {
                                search.update(cx, |search, cx| search.set_value("", window, cx));
                            }
                            cx.notify();
                        }
                    }))
                    .into_any_element(),
            );
        }
        let no_matches = rows.is_empty() && !proxies.is_empty();
        v_flex()
            .debug_selector(|| "proxy-sidebar".to_owned())
            .w(px(220.))
            .flex_shrink_0()
            .h_full()
            .min_h_0()
            .p_3()
            .gap_3()
            .bg(cx.api_surface_low())
            .border_r_1()
            .border_color(cx.api_outline_variant())
            .child(
                h_flex()
                    .px_2()
                    .gap_2()
                    .child(proxy_eyebrow("PROXIES", cx))
                    .when(snapshot.proxies.is_some(), |row| {
                        row.child(proxy_count(proxies.len(), cx))
                    }),
            )
            .child(proxy_search(
                state.proxy_search.as_ref(),
                "Find a proxy…",
                cx,
            ))
            .child(
                v_flex()
                    .id("proxy-list-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_1()
                    .children(rows)
                    .when(no_matches, |list| {
                        list.child(
                            div()
                                .p_3()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("No matching proxies"),
                        )
                    })
                    .when(snapshot.proxies.is_none(), |list| {
                        list.child(
                            div()
                                .p_3()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Proxy list unavailable"),
                        )
                    })
                    .child(
                        div().debug_selector(|| "new-proxy".to_owned()).child(
                            Button::new("new-proxy")
                                .icon(IconName::Plus)
                                .label("New proxy")
                                .small()
                                .ghost()
                                .disabled(!can_create || locked)
                                .tooltip(if !can_create {
                                    "Requires proxies.create"
                                } else {
                                    "Create a named set of host overrides"
                                })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_proxy_name_dialog(None, window, cx)
                                })),
                        ),
                    ),
            )
            .when(
                self.server_management.proxy_rule_editor.is_editing()
                    || self.server_management.proxy_scope_editor.is_editing(),
                |sidebar| {
                    sidebar.child(
                        div()
                            .px_2()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Save or cancel your edit before switching proxies or tabs."),
                    )
                },
            )
            .child(
                v_flex()
                    .flex_shrink_0()
                    .pt_3()
                    .gap_1()
                    .border_t_1()
                    .border_color(cx.api_outline_variant())
                    .child(
                        Button::new("proxy-execution-limits")
                            .icon(IconName::Settings2)
                            .label("Execution limits")
                            .small()
                            .ghost()
                            .selected(state.show_limits)
                            .disabled(locked)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.server_management.proxy_workspace.show_limits = true;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("proxy-routing-help")
                            .icon(IconName::Info)
                            .label("How routing works")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_proxy_routing_help(window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_proxy_detail(
        &self,
        proxy: &ManagementProxy,
        snapshot: &UpstreamManagementSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = &self.server_management.proxy_workspace;
        let locked = self.proxy_navigation_locked();
        let can_update = snapshot.has_permission(PROXIES_UPDATE);
        let can_delete = snapshot.has_permission(PROXIES_DELETE);
        let menu_this = cx.entity().downgrade();
        let menu_proxy = proxy.clone();
        let mut tabs = h_flex()
            .px_6()
            .gap_5()
            .flex_shrink_0()
            .border_b_1()
            .border_color(cx.api_outline_variant());
        for (tab, label, count) in [
            (ProxyTab::Rules, "Rules", proxy.rules.len()),
            (
                ProxyTab::Assignments,
                "Assignments",
                proxy.assignments.len(),
            ),
            (
                ProxyTab::Exclusions,
                "Exclusions",
                proxy.excluded_user_ids.len() + proxy.excluded_role_ids.len(),
            ),
        ] {
            let selected = state.tab == tab;
            let selector = format!("proxy-tab-{}", label.to_lowercase());
            tabs = tabs.child(
                div()
                    .debug_selector(move || selector.clone())
                    .py_1()
                    .border_b_2()
                    .border_color(if selected {
                        cx.theme().primary
                    } else {
                        gpui::transparent_black()
                    })
                    .child(
                        Button::new(SharedString::from(format!("proxy-tab-{label}")))
                            .label(format!("{label}  {count}"))
                            .small()
                            .ghost()
                            .selected(selected)
                            .disabled(locked)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.server_management.proxy_workspace.tab = tab;
                                cx.notify();
                            })),
                    ),
            );
        }
        let content = match state.tab {
            ProxyTab::Rules => self.render_proxy_rules(proxy, snapshot, cx),
            ProxyTab::Assignments => self.render_proxy_assignments_tab(proxy, snapshot, cx),
            ProxyTab::Exclusions => self.render_proxy_exclusions_tab(proxy, snapshot, cx),
        };
        v_flex().debug_selector(|| "proxy-detail".to_owned()).flex_1().min_w_0().min_h_0()
            .when(snapshot.request_execution_settings.as_ref().is_some_and(|settings| settings.mode == RequestExecutionMode::Local), |detail| {
                detail.child(div().debug_selector(|| "proxy-local-execution-notice".to_owned())
                    .px_6().py_2().flex_shrink_0().text_xs()
                    .bg(cx.theme().warning.opacity(0.08)).text_color(cx.theme().warning)
                    .child("Requests run on each user's device. These server proxy rules are saved, but are not applied."))
            })
            .child(h_flex().px_6().py_5().gap_3().flex_shrink_0()
                .child(v_flex().flex_1().min_w_0().gap_2()
                    .child(h_flex().gap_3()
                        .child(div().min_w_0().truncate().text_size(px(21.)).font_semibold().child(proxy.name.clone()))
                        .child(div().px_2().py_0p5().rounded_md().border_1().border_color(cx.api_outline_variant())
                            .text_size(px(10.)).text_color(cx.theme().muted_foreground).child(proxy_scope_summary(proxy))))
                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child(if proxy.assignments.is_empty() {
                        "Not assigned yet. Add a scope to apply these host overrides."
                    } else { "Host overrides for this proxy's assigned scopes." })))
                .child(Button::new("proxy-actions").icon(IconName::EllipsisVertical).small().ghost()
                    .tooltip("Proxy actions").disabled(locked || (!can_update && !can_delete))
                    .dropdown_menu(move |menu, _, _| {
                        let rename_this = menu_this.clone();
                        let rename_proxy = menu_proxy.clone();
                        let delete_this = menu_this.clone();
                        let delete_proxy = menu_proxy.clone();
                        menu.item(PopupMenuItem::new("Rename proxy…").disabled(!can_update).on_click(move |_, window, cx| {
                            if let Some(this) = rename_this.upgrade() {
                                this.update(cx, |this, cx| this.open_proxy_name_dialog(Some(rename_proxy.clone()), window, cx));
                            }
                        }))
                        .item(PopupMenuItem::new("Delete proxy…").disabled(!can_delete).on_click(move |_, window, cx| {
                            if let Some(this) = delete_this.upgrade() {
                                this.update(cx, |this, cx| this.request_delete_proxy(delete_proxy.id.clone(), delete_proxy.name.clone(), window, cx));
                            }
                        }))
                    })))
            .child(tabs).child(content).into_any_element()
    }

    fn render_proxy_rules(
        &self,
        proxy: &ManagementProxy,
        snapshot: &UpstreamManagementSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = &self.server_management.proxy_workspace;
        let query = state
            .rule_search
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default();
        let rules = proxy
            .rules
            .iter()
            .filter(|rule| filter_matches(&query, &[&rule.hostname, &rule.target]))
            .collect::<Vec<_>>();
        let locked = self.proxy_navigation_locked();
        let proxy_id = proxy.id.clone();
        let rows = rules
            .iter()
            .map(|rule| self.render_proxy_rule_row(proxy, rule, cx))
            .collect::<Vec<_>>();
        let list = v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(
                h_flex()
                    .px_5()
                    .py_4()
                    .gap_3()
                    .flex_shrink_0()
                    .child(div().flex_1().min_w_0().max_w(px(300.)).child(proxy_search(
                        state.rule_search.as_ref(),
                        "Filter hostname or target…",
                        cx,
                    )))
                    .child(div().flex_1())
                    .child(
                        div().debug_selector(|| "add-proxy-rule".to_owned()).child(
                            Button::new("add-proxy-rule")
                                .icon(IconName::Plus)
                                .label("Add rule")
                                .small()
                                .primary()
                                .disabled(locked || !snapshot.has_permission(PROXIES_UPDATE))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_proxy_rule_editor(proxy_id.clone(), None, window, cx)
                                })),
                        ),
                    ),
            )
            .child(
                v_flex()
                    .id("proxy-rules-scroll")
                    .debug_selector(|| "proxy-rule-list".to_owned())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_5()
                    .child(
                        h_flex()
                            .gap_3()
                            .px_3()
                            .py_2()
                            .border_b_1()
                            .border_color(cx.api_outline_variant())
                            .text_size(px(10.))
                            .text_color(cx.theme().muted_foreground)
                            .child(div().flex_1().min_w_0().child("Request hostname"))
                            .child(div().w(px(16.)).flex_shrink_0())
                            .child(div().flex_1().min_w_0().child("Connect to"))
                            .child(div().w(px(92.)).flex_shrink_0().child("Host / SNI")),
                    )
                    .children(rows)
                    .when(rules.is_empty(), |list| {
                        list.child(
                            v_flex()
                                .py_8()
                                .gap_2()
                                .items_center()
                                .child(div().text_sm().child(if proxy.rules.is_empty() {
                                    "No rules yet"
                                } else {
                                    "No matching rules"
                                }))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if proxy.rules.is_empty() {
                                            "Add an exact hostname and its connection target."
                                        } else {
                                            "Try another hostname or target."
                                        }),
                                ),
                        )
                    })
                    .child(
                        div()
                            .px_3()
                            .py_3()
                            .text_size(px(10.))
                            .text_color(cx.theme().muted_foreground)
                            .child(if query.trim().is_empty() {
                                format!(
                                    "{} · Exact hostnames only",
                                    count_label(proxy.rules.len(), "rule")
                                )
                            } else {
                                format!(
                                    "{} of {} rules · Exact hostnames only",
                                    rules.len(),
                                    proxy.rules.len()
                                )
                            }),
                    ),
            )
            .child(
                v_flex()
                    .mx_5()
                    .py_4()
                    .gap_2()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(cx.api_outline_variant())
                    .child(proxy_eyebrow("MATCH PRIORITY", cx))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Request  ›  Collection  ›  Workspace  ›  Server"),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(cx.theme().muted_foreground)
                            .child("First matching hostname wins. No matching rule? Normal DNS."),
                    ),
            );
        div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(list)
            .child(self.render_proxy_rule_inspector(proxy, state.selected_rule(proxy), cx))
            .into_any_element()
    }

    fn render_proxy_rule_row(
        &self,
        proxy: &ManagementProxy,
        rule: &HostnameOverride,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (target, kind, behavior) = match parse_proxy_rule_target(&rule.target) {
            Ok(target) => {
                let kind = match &target.scheme {
                    Some(scheme) => format!(
                        "{} · {}",
                        if target.is_ip {
                            "IP address"
                        } else {
                            "Hostname"
                        },
                        scheme.to_uppercase()
                    ),
                    None => if target.is_ip {
                        "IP address"
                    } else {
                        "Hostname"
                    }
                    .to_owned(),
                };
                (
                    target.hostname,
                    kind,
                    if target.is_ip {
                        "Keep original"
                    } else {
                        "Use target"
                    },
                )
            }
            Err(_) => (
                rule.target.clone(),
                "Invalid target".to_owned(),
                "Unavailable",
            ),
        };
        let selected = self
            .server_management
            .proxy_workspace
            .selected_rule_hostname
            .as_deref()
            == Some(&rule.hostname);
        let locked = self.proxy_navigation_locked();
        let hostname = rule.hostname.clone();
        let selector = format!("proxy-rule-{}-{}", proxy.id, rule.hostname);
        h_flex()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .w_full()
            .min_h(px(66.))
            .px_3()
            .py_3()
            .gap_3()
            .border_l_2()
            .border_b_1()
            .border_color(if selected {
                cx.theme().primary.opacity(0.6)
            } else {
                cx.api_outline_variant()
            })
            .bg(if selected {
                cx.theme().primary.opacity(0.10)
            } else {
                gpui::transparent_black()
            })
            .when(!locked, |row| {
                row.cursor_pointer()
                    .hover(|style| style.bg(cx.api_surface_high()))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .font_family(cx.theme().mono_font_family.clone())
                    .child(rule.hostname.clone()),
            )
            .child(
                Icon::new(IconName::ChevronRight)
                    .with_size(px(16.))
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .font_family(cx.theme().mono_font_family.clone())
                            .child(target),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(cx.theme().muted_foreground)
                            .child(kind),
                    ),
            )
            .child(
                div()
                    .w(px(92.))
                    .flex_shrink_0()
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .child(behavior),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.proxy_navigation_locked() {
                    this.server_management
                        .proxy_workspace
                        .selected_rule_hostname = Some(hostname.clone());
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    fn open_proxy_routing_help(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.open_dialog(cx, |dialog, _, cx| {
            dialog.title("How routing works").w(px(540.)).child(
                v_flex().gap_4().text_sm().text_color(cx.theme().muted_foreground)
                    .child("Proxies are named sets of exact hostname overrides, not HTTP or SOCKS proxy endpoints.")
                    .child("For each hostname, the first matching rule wins: saved request → collection (inner to outer) → workspace → server-wide. Without a matching rule, the server uses normal DNS.")
                    .child("Request and collection scopes apply when execution includes an accessible saved request. Ad-hoc requests use workspace and server scopes.")
                    .child("Excluded users and roles skip that proxy and fall through to the next scope. Exclusions are not permissions or deny rules, and do not force direct routing.")
                    .child("IP targets preserve the original HTTP Host and HTTPS SNI. Hostname targets replace them. An explicit target scheme overrides the request scheme; the request port is kept."),
            )
        });
    }
}

fn proxy_search(input: Option<&Entity<InputState>>, placeholder: &str, cx: &mut App) -> AnyElement {
    match input {
        Some(input) => Input::new(input)
            .small()
            .prefix(IconName::Search)
            .into_any_element(),
        None => div()
            .px_2()
            .py_2()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(placeholder.to_owned())
            .into_any_element(),
    }
}

fn proxy_eyebrow(label: &'static str, cx: &mut App) -> AnyElement {
    div()
        .text_size(px(10.))
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(label)
        .into_any_element()
}

fn proxy_count(count: usize, cx: &mut App) -> AnyElement {
    div()
        .px_1p5()
        .rounded_sm()
        .bg(cx.api_surface_high())
        .text_size(px(10.))
        .text_color(cx.theme().muted_foreground)
        .child(count.to_string())
        .into_any_element()
}

fn proxy_empty(title: &str, message: &str, cx: &mut App) -> AnyElement {
    v_flex()
        .flex_1()
        .min_w_0()
        .p_8()
        .items_center()
        .justify_center()
        .gap_2()
        .child(div().text_base().font_semibold().child(title.to_owned()))
        .child(
            div()
                .max_w(px(460.))
                .text_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(message.to_owned()),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests;
