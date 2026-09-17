//! Native configured-rule inspector and draft editor.
//!
//! The owner calls `reconcile` for every snapshot and `finish_save` before
//! installing a mutation's successful snapshot. Failed saves keep the inputs;
//! `set_save_error` attaches the server's error without resetting the draft.
use super::*;
use std::net::IpAddr;

/// Shared by the compact rule table, inspector, and editor preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProxyRuleTarget {
    pub(super) hostname: String,
    pub(super) scheme: Option<String>,
    pub(super) is_ip: bool,
}

impl ProxyRuleTarget {
    pub(super) fn outgoing_hostname<'a>(&'a self, original: &'a str) -> &'a str {
        if self.is_ip { original } else { &self.hostname }
    }

    fn wire_value(&self) -> String {
        match self.scheme.as_deref() {
            Some(scheme) if self.hostname.contains(':') => {
                format!("{scheme}://[{}]", self.hostname)
            }
            Some(scheme) => format!("{scheme}://{}", self.hostname),
            None => self.hostname.clone(),
        }
    }
}

// There is no Rust core validator for these server-owned DTOs. Keep these
// checks aligned with requestproxy/settings.go and proxies.go, not URL's
// browser-oriented normalization (which accepts shorthand IPs and hides
// explicitly specified default ports).
fn normalize_hostname(value: &str) -> Option<String> {
    let value = value.trim();
    let hostname = value
        .strip_suffix('.')
        .unwrap_or(value)
        .to_ascii_lowercase();
    if hostname.is_empty() || hostname.len() > 253 {
        return None;
    }
    hostname
        .split('.')
        .all(|label| {
            let bytes = label.as_bytes();
            !bytes.is_empty()
                && bytes.len() <= 63
                && bytes[0].is_ascii_alphanumeric()
                && bytes[bytes.len() - 1].is_ascii_alphanumeric()
                && bytes
                    .iter()
                    .all(|b| b.is_ascii_alphanumeric() || *b == b'-')
        })
        .then_some(hostname)
}

pub(super) fn parse_proxy_rule_target(value: &str) -> Result<ProxyRuleTarget, String> {
    let invalid = || {
        "Enter a hostname or IP, optionally prefixed with http:// or https://, without a port or path."
            .to_owned()
    };
    let value = value.trim();
    if value.is_empty() || value.len() > 512 {
        return Err(invalid());
    }
    let (scheme, host) = if let Some((scheme, authority)) = value.split_once("://") {
        let scheme = scheme.to_ascii_lowercase();
        if !matches!(scheme.as_str(), "http" | "https") {
            return Err(invalid());
        }
        let host = authority.strip_suffix('/').unwrap_or(authority);
        if host.chars().any(char::is_whitespace) {
            return Err(invalid());
        }
        // Brackets are required for IPv6 in a scheme-bearing target.
        if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
            return Err(invalid());
        }
        if host.starts_with('[')
            && host
                .strip_prefix('[')
                .and_then(|host| host.strip_suffix(']'))
                .is_none_or(|host| host.parse::<std::net::Ipv6Addr>().is_err())
        {
            return Err(invalid());
        }
        (Some(scheme), host)
    } else {
        (None, value)
    };
    let ip_host = host.trim_matches(['[', ']']);
    let (hostname, is_ip) = match ip_host.parse::<IpAddr>() {
        Ok(ip) => (ip.to_string(), true),
        Err(_) => (normalize_hostname(host).ok_or_else(invalid)?, false),
    };
    Ok(ProxyRuleTarget {
        hostname,
        scheme,
        is_ip,
    })
}

#[derive(Clone)]
struct RuleDraft {
    proxy_id: String,
    original: Option<HostnameOverride>,
    hostname: Entity<InputState>,
    target: Entity<InputState>,
    scheme: Option<String>,
    conflict: Option<String>,
    error: Option<String>,
    pending: Option<HostnameOverride>,
    // Dropped with the editor; never detached into the application's lifetime.
    _subscriptions: Rc<Vec<Subscription>>,
}

#[derive(Clone, Default)]
pub(super) struct ProxyRuleEditorState {
    draft: Option<RuleDraft>,
}

impl std::fmt::Debug for ProxyRuleEditorState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyRuleEditorState")
            .field("is_editing", &self.is_editing())
            .finish()
    }
}

impl ProxyRuleEditorState {
    pub(super) fn is_editing(&self) -> bool {
        self.draft.is_some()
    }

    pub(super) fn clear(&mut self) {
        self.draft = None;
    }

    pub(super) fn reconcile(&mut self, snapshot: &UpstreamManagementSnapshot) {
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        let proxy = snapshot
            .proxies
            .as_ref()
            .and_then(|proxies| proxies.iter().find(|proxy| proxy.id == draft.proxy_id));
        draft.conflict = if !snapshot.has_permission(PROXIES_UPDATE) {
            Some("You no longer have permission to edit rules. Your draft is preserved.".into())
        } else if let Some(proxy) = proxy {
            check_original(&proxy.rules, draft.original.as_ref()).err()
        } else {
            Some("This proxy is no longer available. Your draft is preserved.".into())
        };
    }

    /// Only a submission made by this editor can finish it. Call before
    /// `set_snapshot` so our own successful edit isn't mistaken for a conflict.
    pub(super) fn finish_save(&mut self, success: bool) -> Option<(String, String)> {
        let draft = self.draft.as_mut()?;
        let pending = draft.pending.take()?;
        if success {
            let selection = (draft.proxy_id.clone(), pending.hostname);
            self.clear();
            Some(selection)
        } else {
            draft.error = Some("The rule could not be saved. Your draft is preserved.".into());
            None
        }
    }

    pub(super) fn set_save_error(&mut self, error: String) {
        if let Some(draft) = self.draft.as_mut() {
            draft.error = Some(error);
        }
    }
}

fn check_original(
    rules: &[HostnameOverride],
    original: Option<&HostnameOverride>,
) -> Result<(), String> {
    if let Some(original) = original {
        match rules.iter().find(|rule| rule.hostname == original.hostname) {
            Some(current) if current == original => {}
            Some(_) => return Err("This rule changed on the server. Cancel and reopen it to edit the latest version; your draft has not been overwritten.".into()),
            None => return Err("This rule was deleted on the server. Cancel and create a new rule if needed; your draft has not been overwritten.".into()),
        }
    }
    Ok(())
}

fn updated_rules(
    latest: &[HostnameOverride],
    original: Option<&HostnameOverride>,
    hostname: &str,
    target: &str,
    scheme: Option<&str>,
) -> Result<(Vec<HostnameOverride>, HostnameOverride), String> {
    check_original(latest, original)?;
    let hostname = normalize_hostname(hostname)
        .filter(|host| host.parse::<IpAddr>().is_err())
        .ok_or("Enter an exact DNS hostname, without a scheme, port, path, wildcard, or IP.")?;
    if target.contains("://") {
        return Err(
            "Enter only a hostname or IP in Connect to; choose the outgoing scheme separately."
                .into(),
        );
    }
    let mut target = parse_proxy_rule_target(target)?;
    target.scheme = scheme.map(str::to_owned);
    let rule = HostnameOverride {
        hostname,
        target: target.wire_value(),
    };
    let mut rules: Vec<_> = latest
        .iter()
        .filter(|entry| original.is_none_or(|original| entry.hostname != original.hostname))
        .cloned()
        .collect();
    if rules
        .iter()
        .any(|entry| normalize_hostname(&entry.hostname).as_deref() == Some(rule.hostname.as_str()))
    {
        return Err("This proxy already has a rule for that exact hostname.".into());
    }
    if rules.len() >= 256 {
        return Err("A proxy can contain at most 256 rules.".into());
    }
    rules.push(rule.clone());
    rules.sort_by(|left, right| left.hostname.cmp(&right.hostname));
    Ok((rules, rule))
}

impl ApiTester {
    pub(in crate::app::server_management) fn open_proxy_rule_editor(
        &mut self,
        proxy_id: String,
        existing: Option<HostnameOverride>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.server_management.proxy_rule_editor.is_editing()
            || self.server_management.status.busy()
            || !self
                .server_management
                .snapshot
                .as_ref()
                .is_some_and(|s| s.has_permission(PROXIES_UPDATE))
            || self.management_proxy(&proxy_id).is_none()
        {
            return;
        }
        let parsed = existing
            .as_ref()
            .and_then(|rule| parse_proxy_rule_target(&rule.target).ok());
        let hostname = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("api.example.com")
                .default_value(
                    existing
                        .as_ref()
                        .map(|rule| rule.hostname.clone())
                        .unwrap_or_default(),
                )
        });
        let target = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Hostname or IP address")
                .default_value(
                    parsed
                        .as_ref()
                        .map(|target| target.hostname.clone())
                        .or_else(|| existing.as_ref().map(|rule| rule.target.clone()))
                        .unwrap_or_default(),
                )
        });
        let subscriptions = [&hostname, &target]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |_, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                })
            })
            .collect();
        self.server_management.proxy_rule_editor.draft = Some(RuleDraft {
            proxy_id,
            original: existing,
            hostname: hostname.clone(),
            target,
            scheme: parsed.and_then(|target| target.scheme),
            conflict: None,
            error: None,
            pending: None,
            _subscriptions: Rc::new(subscriptions),
        });
        if let Some(snapshot) = self.server_management.snapshot.as_ref() {
            self.server_management.proxy_rule_editor.reconcile(snapshot);
        }
        hostname.read(cx).focus_handle(cx).focus(window);
        cx.notify();
    }

    fn save_proxy_rule_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.server_management.status.busy() {
            return;
        }
        let Some(snapshot) = self.server_management.snapshot.as_ref() else {
            return;
        };
        self.server_management.proxy_rule_editor.reconcile(snapshot);
        let Some(draft) = self.server_management.proxy_rule_editor.draft.as_ref() else {
            return;
        };
        if draft.conflict.is_some() || draft.pending.is_some() {
            return;
        }
        let proxy_id = draft.proxy_id.clone();
        let Some(proxy) = self.management_proxy(&proxy_id) else {
            return;
        };
        let result = updated_rules(
            &proxy.rules,
            draft.original.as_ref(),
            &draft.hostname.read(cx).value(),
            &draft.target.read(cx).value(),
            draft.scheme.as_deref(),
        );
        match result {
            Ok((rules, rule)) => {
                let draft = self
                    .server_management
                    .proxy_rule_editor
                    .draft
                    .as_mut()
                    .unwrap();
                draft.pending = Some(rule);
                draft.error = None;
                self.run_management_mutation(
                    ManagementMutation::UpdateProxyRules { proxy_id, rules },
                    window,
                    cx,
                );
                // Mutation startup can fail before spawning (missing credentials,
                // invalid profile). Do not strand the editor in a pending state.
                if !self.server_management.status.busy() {
                    self.server_management.proxy_rule_editor.finish_save(false);
                    if let Some(error) = self.settings_notice.clone() {
                        self.server_management
                            .proxy_rule_editor
                            .set_save_error(error);
                    }
                }
            }
            Err(error) => self
                .server_management
                .proxy_rule_editor
                .set_save_error(error),
        }
        cx.notify();
    }

    pub(in crate::app::server_management) fn render_proxy_rule_inspector(
        &self,
        proxy: &ManagementProxy,
        entry: Option<&HostnameOverride>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(editor) = self.render_active_proxy_rule_editor(cx) {
            return editor;
        }
        let panel = rule_panel(cx);
        let Some(entry) = entry else {
            return panel.child(v_flex().p_5().gap_3()
                .child(div().text_sm().font_semibold().child("Rule details"))
                .child(div().text_sm().text_color(cx.theme().muted_foreground)
                    .child(if proxy.rules.is_empty() { "No rules yet. Add an exact hostname override to configure this proxy." } else { "Select a rule to inspect its configured behavior." })))
                .into_any_element();
        };
        let can_update = self
            .server_management
            .snapshot
            .as_ref()
            .is_some_and(|s| s.has_permission(PROXIES_UPDATE));
        let disabled = !can_update || self.server_management.status.busy();
        let edit_proxy = proxy.id.clone();
        let edit_entry = entry.clone();
        let delete_proxy = proxy.id.clone();
        let delete_hostname = entry.hostname.clone();
        let this = cx.entity().downgrade();
        panel
            .child(
                h_flex()
                    .p_5()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("RULE DETAILS"),
                    )
                    .child(
                        Button::new("proxy-rule-menu")
                            .label("⋯")
                            .ghost()
                            .small()
                            .disabled(disabled)
                            .tooltip("Rule actions")
                            .dropdown_menu(move |menu, _, _| {
                                let this = this.clone();
                                let proxy_id = delete_proxy.clone();
                                let hostname = delete_hostname.clone();
                                menu.item(PopupMenuItem::new("Delete rule…").on_click(
                                    move |_, window, cx| {
                                        if let Some(this) = this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.request_delete_proxy_rule(
                                                    proxy_id.clone(),
                                                    hostname.clone(),
                                                    window,
                                                    cx,
                                                )
                                            });
                                        }
                                    },
                                ))
                            }),
                    ),
            )
            .child(
                v_flex()
                    .id("proxy-rule-inspector-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_5()
                    .pb_5()
                    .gap_5()
                    .child(
                        div()
                            .text_lg()
                            .font_semibold()
                            .child(entry.hostname.clone()),
                    )
                    .child(rule_behavior(entry, cx))
                    .when(!can_update, |view| {
                        view.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(
                                    "Read only · You do not have permission to update proxy rules.",
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .debug_selector(|| "proxy-rule-edit-action".to_owned())
                    .p_5()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(cx.api_outline_variant())
                    .child(
                        Button::new("edit-proxy-rule")
                            .label("Edit rule")
                            .w_full()
                            .disabled(disabled)
                            .tooltip(if can_update {
                                "Edit this rule's configured behavior"
                            } else {
                                "Requires permission to update proxies"
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_proxy_rule_editor(
                                    edit_proxy.clone(),
                                    Some(edit_entry.clone()),
                                    window,
                                    cx,
                                );
                            })),
                    ),
            )
            .into_any_element()
    }

    /// Render this even if a refresh removed the selected proxy. The draft and
    /// its conflict message must remain reachable, including the Cancel action.
    pub(in crate::app::server_management) fn render_active_proxy_rule_editor(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let draft = self.server_management.proxy_rule_editor.draft.as_ref()?;
        Some(
            rule_panel(cx)
                .child(self.render_proxy_rule_editor(draft, cx))
                .into_any_element(),
        )
    }

    fn render_proxy_rule_editor(&self, draft: &RuleDraft, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.server_management.status.busy() || draft.pending.is_some();
        let hostname = draft.hostname.read(cx).value().to_string();
        let target = draft.target.read(cx).value().to_string();
        let validation = self.management_proxy(&draft.proxy_id).map(|proxy| {
            updated_rules(
                &proxy.rules,
                draft.original.as_ref(),
                &hostname,
                &target,
                draft.scheme.as_deref(),
            )
        });
        let valid = matches!(validation, Some(Ok(_))) && draft.conflict.is_none();
        let preview = parse_proxy_rule_target(&target)
            .ok()
            .filter(|target| target.scheme.is_none())
            .map(|mut target| {
                target.scheme = draft.scheme.clone();
                HostnameOverride {
                    hostname: normalize_hostname(&hostname)
                        .unwrap_or_else(|| "the request hostname".into()),
                    target: target.wire_value(),
                }
            });
        let selected_scheme = draft.scheme.clone();
        let proxy_id = draft.proxy_id.clone();
        let this = cx.entity().downgrade();
        let choices = Button::new("proxy-rule-scheme")
            .label(match selected_scheme.as_deref() {
                Some("http") => "HTTP",
                Some("https") => "HTTPS",
                _ => "Keep request scheme",
            })
            .small()
            .outline()
            .w_full()
            .disabled(busy)
            .dropdown_menu(move |menu, _, _| {
                [
                    ("Keep request scheme", None),
                    ("HTTP", Some("http")),
                    ("HTTPS", Some("https")),
                ]
                .into_iter()
                .fold(menu, |menu, (label, scheme)| {
                    let this = this.clone();
                    let proxy_id = proxy_id.clone();
                    menu.item(
                        PopupMenuItem::new(label)
                            .checked(selected_scheme.as_deref() == scheme)
                            .on_click(move |_, _, cx| {
                                if let Some(this) = this.upgrade() {
                                    this.update(cx, |this, cx| {
                                        if this.server_management.status.busy() {
                                            return;
                                        }
                                        if let Some(draft) = this
                                            .server_management
                                            .proxy_rule_editor
                                            .draft
                                            .as_mut()
                                            .filter(|draft| {
                                                draft.proxy_id == proxy_id
                                                    && draft.pending.is_none()
                                            })
                                        {
                                            draft.scheme = scheme.map(str::to_owned);
                                            cx.notify();
                                        }
                                    });
                                }
                            }),
                    )
                })
            });
        v_flex()
            .debug_selector(|| "proxy-rule-editor".to_owned())
            .h_full()
            .min_h_0()
            .child(
                v_flex()
                    .p_5()
                    .gap_2()
                    .flex_shrink_0()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(if draft.original.is_some() {
                                "EDIT RULE · DRAFT"
                            } else {
                                "NEW RULE · DRAFT"
                            }),
                    )
                    .child(
                        div()
                            .text_lg()
                            .font_semibold()
                            .child(if draft.original.is_some() {
                                "Edit host override"
                            } else {
                                "Add host override"
                            }),
                    ),
            )
            .child(
                v_flex()
                    .id("proxy-rule-editor-fields")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_5()
                    .pb_5()
                    .gap_4()
                    .child(management_dialog_field(
                        "REQUEST HOSTNAME",
                        Input::new(&draft.hostname).disabled(busy),
                    ))
                    .child(editor_note(
                        "Exact hostname. No scheme, port, path, or wildcards.",
                        cx,
                    ))
                    .child(management_dialog_field(
                        "CONNECT TO",
                        Input::new(&draft.target).disabled(busy),
                    ))
                    .child(editor_note("Hostname or IP only. No target port.", cx))
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("OUTGOING SCHEME"),
                            )
                            .child(choices),
                    )
                    .when_some(preview, |view, rule| {
                        view.child(rule_editor_preview(&rule, cx))
                    })
                    .when_some(draft.conflict.clone(), |view, error| {
                        view.child(editor_error(error, cx))
                    })
                    .when_some(draft.error.clone(), |view, error| {
                        view.child(editor_error(error, cx))
                    })
                    .when_some(
                        validation
                            .and_then(Result::err)
                            .filter(|_| draft.conflict.is_none()),
                        |view, error| view.child(editor_error(error, cx)),
                    ),
            )
            .child(
                v_flex()
                    .debug_selector(|| "proxy-rule-editor-footer".to_owned())
                    .p_5()
                    .gap_3()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(cx.api_outline_variant())
                    .child(editor_note(
                        "Saving updates this proxy for its assigned scopes.",
                        cx,
                    ))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .debug_selector(|| "cancel-proxy-rule".to_owned())
                                    .child(
                                        Button::new("cancel-proxy-rule")
                                            .label("Cancel")
                                            .disabled(busy)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.server_management.proxy_rule_editor.clear();
                                                cx.notify();
                                            })),
                                    ),
                            )
                            .child(
                                div().debug_selector(|| "save-proxy-rule".to_owned()).child(
                                    Button::new("save-proxy-rule")
                                        .label(if busy { "Saving…" } else { "Save rule" })
                                        .primary()
                                        .disabled(busy || !valid)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.save_proxy_rule_editor(window, cx)
                                        })),
                                ),
                            ),
                    ),
            )
            .into_any_element()
    }
}

fn rule_panel(cx: &App) -> gpui::Div {
    v_flex()
        .debug_selector(|| "proxy-rule-inspector".to_owned())
        .w(px(312.))
        .h_full()
        .flex_shrink_0()
        .min_h_0()
        .border_l_1()
        .border_color(cx.api_outline_variant())
        .bg(cx.api_surface_low())
}

fn editor_note(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
        .into_any_element()
}

fn editor_error(text: String, cx: &App) -> AnyElement {
    div()
        .text_sm()
        .text_color(cx.theme().danger)
        .child(text)
        .into_any_element()
}

fn detail(label: &'static str, value: impl Into<SharedString>, cx: &App) -> AnyElement {
    h_flex()
        .items_start()
        .justify_between()
        .gap_3()
        .child(
            div()
                .flex_shrink_0()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(
            div()
                .min_w_0()
                .text_right()
                .text_xs()
                .font_family(cx.theme().mono_font_family.clone())
                .child(value.into()),
        )
        .into_any_element()
}

fn rule_editor_preview(rule: &HostnameOverride, cx: &App) -> AnyElement {
    let Ok(target) = parse_proxy_rule_target(&rule.target) else {
        return editor_note("Enter a target to see its host behavior.", cx);
    };
    v_flex()
        .p_3()
        .gap_2()
        .rounded_md()
        .bg(cx.theme().primary.opacity(0.08))
        .child(
            div()
                .text_xs()
                .font_semibold()
                .child("Derived host behavior"),
        )
        .child(editor_note(
            format!(
                "{} {} as HTTP Host and HTTPS SNI.",
                if target.is_ip { "Keep" } else { "Use" },
                target.outgoing_hostname(&rule.hostname),
            ),
            cx,
        ))
        .into_any_element()
}

fn rule_behavior(rule: &HostnameOverride, cx: &App) -> AnyElement {
    let Ok(target) = parse_proxy_rule_target(&rule.target) else {
        return editor_error(
            "This rule has an unrecognized target. Edit it to correct the configuration.".into(),
            cx,
        );
    };
    let host = target.outgoing_hostname(&rule.hostname).to_owned();
    v_flex().gap_4()
        .child(editor_note(if target.is_ip { "IP target" } else { "Hostname target" }, cx))
        .child(detail("Match exact hostname", rule.hostname.clone(), cx))
        .child(detail("Connect to", target.hostname, cx))
        .child(v_flex().p_3().gap_3().rounded_md().bg(cx.api_surface_container())
            .child(div().text_xs().font_semibold().child("CONFIGURED OUTGOING BEHAVIOR"))
            .child(detail("HTTP Host", host.clone(), cx))
            .child(detail("HTTPS SNI", host, cx))
            .child(detail("Scheme", target.scheme.as_deref().map(str::to_ascii_uppercase).unwrap_or_else(|| "From request".into()), cx))
            .child(detail("Port", "Explicit request port; otherwise scheme default", cx))
            .child(editor_note(if target.is_ip {
                "An IP target changes the connection address and preserves the original host. HTTP Host includes the request port when required; SNI applies only to HTTPS."
            } else {
                "A hostname target rewrites the outgoing URL host, HTTP Host, and HTTPS SNI. HTTP Host includes the request port when required; SNI applies only to HTTPS."
            }, cx))
            .when(target.scheme.is_some(), |view| view.child(editor_note("The configured scheme overrides the request scheme and also permits matching request URLs to omit their scheme.", cx))))
        .child(editor_note("Configuration only — not a sent request or a live wire preview.", cx))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(hostname: &str, target: &str) -> HostnameOverride {
        HostnameOverride {
            hostname: hostname.into(),
            target: target.into(),
        }
    }

    #[test]
    fn parses_target_kind_scheme_and_ipv6_without_inventing_ports() {
        for (value, host, scheme, ip, wire) in [
            (
                " HTTPS://Gateway.Internal./ ",
                "gateway.internal",
                Some("https"),
                false,
                "https://gateway.internal",
            ),
            ("10.0.0.25", "10.0.0.25", None, true, "10.0.0.25"),
            ("https://[::1]", "::1", Some("https"), true, "https://[::1]"),
            ("[::1]", "::1", None, true, "::1"),
            (
                "gateway.internal",
                "gateway.internal",
                None,
                false,
                "gateway.internal",
            ),
        ] {
            let target = parse_proxy_rule_target(value).unwrap();
            assert_eq!(target.hostname, host);
            assert_eq!(target.scheme.as_deref(), scheme);
            assert_eq!(target.is_ip, ip);
            assert_eq!(target.wire_value(), wire);
            assert_eq!(
                target.outgoing_hostname("api.internal"),
                if ip { "api.internal" } else { host },
            );
        }
        for invalid in [
            "https://host:443",
            "http://host:80",
            "host/path",
            "https://host/path",
            "https://user@host",
            "https://host?q=1",
            "https://host#x",
            "ftp://host",
            "https://::1",
            "https://[[::1]]",
            "https:// host",
            "*.internal",
            "",
        ] {
            assert!(parse_proxy_rule_target(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn validates_exact_hostname_duplicates_and_separate_scheme() {
        let existing = vec![rule("api.internal", "10.0.0.1")];
        for hostname in [
            "",
            "*.internal",
            "https://api.internal",
            "host:443",
            "host/path",
            "127.0.0.1",
            "bad_thing",
        ] {
            assert!(updated_rules(&[], None, hostname, "gateway", None).is_err());
        }
        assert!(updated_rules(&existing, None, " API.Internal. ", "gateway", None).is_err());
        assert!(updated_rules(&[], None, "api", "https://gateway", None).is_err());
        let (_, saved) = updated_rules(&[], None, " API.Internal. ", "::1", Some("https")).unwrap();
        assert_eq!(saved, rule("api.internal", "https://[::1]"));
    }

    #[test]
    fn rebases_on_latest_array_but_rejects_modified_or_deleted_original() {
        let original = rule("api", "10.0.0.1");
        let unrelated = rule("other", "gateway");
        let latest = vec![original.clone(), unrelated.clone()];
        let (updated, _) =
            updated_rules(&latest, Some(&original), "renamed", "10.0.0.2", None).unwrap();
        assert!(updated.contains(&unrelated));
        assert!(!updated.contains(&original));
        assert!(
            updated_rules(
                &[rule("api", "changed")],
                Some(&original),
                "api",
                "mine",
                None
            )
            .is_err()
        );
        assert!(updated_rules(&[], Some(&original), "api", "mine", None).is_err());
        assert!(updated_rules(&latest, Some(&original), "other", "mine", None).is_err());
    }

    #[test]
    fn default_state_has_no_submission_to_finish() {
        let mut state = ProxyRuleEditorState::default();
        assert!(!state.is_editing());
        assert!(state.finish_save(true).is_none());
        assert!(state.finish_save(false).is_none());
        state.clear();
        assert!(!state.is_editing());
    }

    struct EditorHarness;

    impl Render for EditorHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    fn snapshot(rules: Vec<HostnameOverride>) -> UpstreamManagementSnapshot {
        let now = Utc::now();
        UpstreamManagementSnapshot {
            current_user: ManagementUser {
                id: "user".into(),
                email: "user@example.com".into(),
                display_name: "User".into(),
                active: true,
                roles: vec![ManagementRole {
                    id: "role".into(),
                    name: "Role".into(),
                    description: String::new(),
                    system: false,
                    permissions: vec![crate::core::ManagementPermission {
                        key: PROXIES_UPDATE.into(),
                        description: String::new(),
                    }],
                    created_by: None,
                    created_at: now,
                    updated_at: now,
                }],
                created_by: None,
                created_at: now,
                updated_at: now,
            },
            profiles: Vec::new(),
            users: None,
            roles: None,
            permissions: None,
            workspaces: None,
            request_execution_settings: None,
            proxies: Some(vec![ManagementProxy {
                id: "proxy".into(),
                name: "Proxy".into(),
                rules,
                assignments: Vec::new(),
                excluded_user_ids: Vec::new(),
                excluded_role_ids: Vec::new(),
                created_at: now,
                updated_at: now,
            }]),
        }
    }

    #[gpui::test]
    fn failed_save_and_refresh_preserve_inputs_until_success(cx: &mut gpui::TestAppContext) {
        let original = rule("api", "10.0.0.1");
        let saved = rule("renamed", "https://gateway");
        let mut state = ProxyRuleEditorState::default();
        let (_, visual) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            state.draft = Some(RuleDraft {
                proxy_id: "proxy".into(),
                original: Some(original.clone()),
                hostname: cx.new(|cx| InputState::new(window, cx).default_value("renamed")),
                target: cx.new(|cx| InputState::new(window, cx).default_value("gateway")),
                scheme: Some("https".into()),
                conflict: None,
                error: None,
                pending: Some(saved.clone()),
                _subscriptions: Rc::new(Vec::new()),
            });
            EditorHarness
        });
        assert!(state.finish_save(false).is_none());
        state.set_save_error("Server rejected this change".into());
        state.reconcile(&snapshot(vec![
            original.clone(),
            rule("other", "elsewhere"),
        ]));
        visual.update(|_, cx| {
            let draft = state.draft.as_ref().unwrap();
            assert_eq!(draft.hostname.read(cx).value().as_str(), "renamed");
            assert_eq!(draft.target.read(cx).value().as_str(), "gateway");
            assert_eq!(draft.error.as_deref(), Some("Server rejected this change"));
            assert!(draft.conflict.is_none());
            assert!(draft.pending.is_none());
        });
        state.reconcile(&snapshot(vec![rule("api", "external-change")]));
        assert!(state.draft.as_ref().unwrap().conflict.is_some());
        state.reconcile(&snapshot(Vec::new()));
        assert!(
            state
                .draft
                .as_ref()
                .unwrap()
                .conflict
                .as_ref()
                .unwrap()
                .contains("deleted")
        );
        let mut unavailable = snapshot(vec![original]);
        unavailable.proxies = None;
        state.reconcile(&unavailable);
        assert!(state.is_editing());
        assert!(
            state
                .draft
                .as_ref()
                .unwrap()
                .conflict
                .as_ref()
                .unwrap()
                .contains("available")
        );
        unavailable.current_user.roles.clear();
        state.reconcile(&unavailable);
        assert!(
            state
                .draft
                .as_ref()
                .unwrap()
                .conflict
                .as_ref()
                .unwrap()
                .contains("permission")
        );
        state.draft.as_mut().unwrap().pending = Some(saved);
        assert_eq!(
            state.finish_save(true),
            Some(("proxy".into(), "renamed".into()))
        );
        assert!(!state.is_editing());
    }
}
