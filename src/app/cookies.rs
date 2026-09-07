use super::*;
use crate::core::CookieEntry;

impl ApiTester {
    pub(super) fn render_cookie_manager(
        &self,
        id: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let value_header_group: SharedString = format!("{id}-cookie-value-header").into();
        let jar = self.cookie_jar.clone();
        let entries = jar.entries();
        let selected = entries
            .iter()
            .find(|entry| self.selected_cookie.as_ref() == Some(&cookie_key(entry)))
            .cloned();
        let edit_entry = selected.clone();
        let delete_entry = selected.clone();
        let toggle_jar = jar.clone();
        let reload_jar = jar.clone();
        let delete_jar = jar.clone();
        let reset_jar = jar.clone();

        v_flex()
            .id(id.clone())
            .size_full()
            .min_h_0()
            .rounded_lg()
            .border_1()
            .border_color(cx.api_outline_variant())
            .overflow_hidden()
            .bg(cx.api_surface())
            .child(h_flex().h(px(42.)).flex_shrink_0().px_3().gap_2()
                .bg(cx.api_surface_low())
                .child(div().text_sm().font_semibold().child("Cookies"))
                .child(div().text_xs().text_color(cx.theme().muted_foreground)
                    .child(format!("{} saved · {}", entries.len(), self.active_workspace_name()))))
            .child(h_flex().h(px(34.)).flex_shrink_0()
                .bg(cx.api_surface_low()).text_xs().font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child(cookie_table_cell("NAME".to_owned(), cx))
                .child(h_flex().id("cookie-value-header").debug_selector(|| "cookie-value-header".to_owned()).group(value_header_group.clone()).flex_1().min_w_0().h_full().px_3().gap_2().justify_between()
                    .border_l_1().border_color(cx.api_outline_variant())
                    .child("VALUE")
                    .child(div().flex_shrink_0().invisible().group_hover(value_header_group, |style| style.visible())
                        .child(Button::new("toggle-cookie-values")
                        .debug_selector(|| "toggle-cookie-values".to_owned())
                        .icon(if self.cookie_values_visible { IconName::EyeOff } else { IconName::Eye })
                        .tooltip(if self.cookie_values_visible { "Hide cookie values" } else { "Show cookie values" })
                        .xsmall().ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.cookie_values_visible = !this.cookie_values_visible;
                            this.cookie_value_visibility.clear();
                            cx.notify();
                        })))))
                .children(["DOMAIN", "PATH"].into_iter()
                    .map(|label| cookie_table_cell(label.to_owned(), cx))))
            .child(v_flex().id("cookie-list").flex_1().min_h_0().overflow_y_scroll()
                .when(entries.is_empty(), |view| view.child(
                    div().p_4().text_sm().text_color(cx.theme().muted_foreground)
                        .child("No cookies in this workspace.")))
                .children(entries.into_iter().enumerate().map(|(index, entry)| {
                    let key = cookie_key(&entry);
                    let is_selected = self.selected_cookie.as_ref() == Some(&key);
                    let visible = self.cookie_value_visibility.get(&key).copied().unwrap_or(self.cookie_values_visible);
                    let toggle_key = key.clone();
                    let group: SharedString = format!("{id}-cookie-row-{index}").into();
                    h_flex().id(("cookie-row", index)).group(group.clone())
                        .debug_selector(move || format!("cookie-row-{index}"))
                        .h(px(44.)).w_full().flex_shrink_0().text_sm()
                        .border_t_1().border_color(cx.api_outline_variant())
                        .bg(if is_selected { cx.theme().accent } else { cx.api_surface() })
                        .when(is_selected, |view| view.text_color(cx.theme().accent_foreground))
                        .hover(|style| style.bg(cx.theme().accent))
                        .cursor_pointer()
                        .child(cookie_table_cell(entry.name, cx))
                        .child(h_flex().flex_1().min_w_0().h_full().px_3().gap_2()
                            .border_l_1().border_color(cx.api_outline_variant())
                            .child(div().flex_1().min_w_0().truncate().child(if visible { entry.value } else { "Value hidden".to_owned() }))
                            .child(div().flex_shrink_0().invisible().group_hover(group, |style| style.visible())
                                .child(Button::new(("toggle-cookie-value", index))
                                    .debug_selector(move || format!("toggle-cookie-value-{index}"))
                                    .icon(if visible { IconName::EyeOff } else { IconName::Eye })
                                    .tooltip(if visible { "Hide value" } else { "Show value" })
                                    .xsmall().ghost()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.cookie_value_visibility.insert(toggle_key.clone(), !visible);
                                        cx.notify();
                                    })))))
                        .child(cookie_table_cell(entry.domain, cx))
                        .child(cookie_table_cell(entry.path, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected_cookie = Some(key.clone());
                            cx.notify();
                        }))
                })))
            .child(v_flex().flex_shrink_0().px_3().py_2().gap_2()
                .border_t_1().border_color(cx.api_outline_variant())
                .child(h_flex().gap_2().flex_wrap()
                    .child(Button::new("add-cookie").debug_selector(|| "add-cookie".to_owned())
                        .label("Add cookie").icon(IconName::Plus).small().ghost()
                        .on_click(cx.listener(|this, _, window, cx| this.open_cookie_editor(None, window, cx))))
                    .child(Button::new("edit-cookie").label("View / edit").small().ghost()
                        .disabled(selected.is_none())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if let Some(entry) = edit_entry.clone() { this.open_cookie_editor(Some(entry), window, cx); }
                        })))
                    .child(Button::new("delete-cookie").debug_selector(|| "delete-cookie".to_owned()).label("Delete").small().danger().outline()
                        .disabled(selected.is_none())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if let Some(entry) = delete_entry.as_ref() {
                                if let Err(error) = delete_jar.delete(entry) {
                                    window.push_notification(Notification::error(error), cx);
                                } else { this.selected_cookie = None; }
                                cx.notify();
                            }
                        })))
                    .child(Button::new("toggle-cookies").debug_selector(|| "toggle-cookies".to_owned())
                        .label(if jar.enabled() { "Disable automatic cookies" } else { "Enable automatic cookies" })
                        .small().ghost()
                        .on_click(cx.listener(move |_, _, window, cx| {
                            if let Err(error) = toggle_jar.set_enabled(!toggle_jar.enabled()) {
                                window.push_notification(Notification::error(error), cx);
                            }
                            cx.notify();
                        })))
                    .when(jar.is_remote(), |view| view.child(Button::new("reload-cookies")
                        .label("Reload from server").small().ghost()
                        .on_click(cx.listener(move |_, _, window, cx| {
                            if let Err(error) = reload_jar.reload() { window.push_notification(Notification::error(error), cx); }
                            cx.notify();
                        }))))
                    .child(Button::new("clear-cookies").debug_selector(|| "clear-cookies".to_owned())
                        .label("Clear / reset jar").small().danger().outline()
                        .on_click(cx.listener(move |_, _, window, cx| {
                            let jar = reset_jar.clone();
                            let this = cx.entity().downgrade();
                            window.open_dialog(cx, move |dialog, _, _| {
                                let jar = jar.clone(); let this = this.clone();
                                dialog.title("Clear this workspace's cookies?").confirm()
                                    .child("This removes all saved cookies, including unreadable storage. Other workspaces are unaffected.")
                                    .on_ok(move |_, window, cx| {
                                        if let Err(error) = jar.clear() { window.push_notification(Notification::error(error), cx); return false; }
                                        if let Some(this) = this.upgrade() { this.update(cx, |this, cx| { this.selected_cookie = None; cx.notify(); }); }
                                        true
                                    })
                            });
                        }))))
                .when(jar.syncing(), |view| view.child(div().text_xs()
                    .text_color(cx.theme().muted_foreground).child("Synchronizing encrypted cookie storage…")))
                .when_some(jar.warning(), |view, warning| view.child(div().text_xs()
                    .text_color(cx.theme().danger).child(warning))))
            .into_any_element()
    }

    pub(super) fn cookie_tab_label(&self) -> String {
        if self.cookie_jar.syncing() {
            "Cookies…"
        } else if self.cookie_jar.warning().is_some() {
            "Cookies ⚠"
        } else if self.cookie_jar.enabled() {
            "Cookies"
        } else {
            "Cookies off"
        }
        .to_owned()
    }

    fn open_cookie_editor(
        &mut self,
        entry: Option<CookieEntry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let origin = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://api.example.com/")
                .default_value(
                    entry
                        .as_ref()
                        .map(|entry| entry.origin.clone())
                        .unwrap_or_default(),
                )
        });
        let header = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("session=value; Path=/; Secure; HttpOnly")
                .default_value(
                    entry
                        .as_ref()
                        .map(|entry| entry.header.clone())
                        .unwrap_or_default(),
                )
        });
        let jar = self.cookie_jar.clone();
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let origin_ok = origin.clone(); let header_ok = header.clone(); let jar = jar.clone(); let old = entry.clone(); let this = this.clone();
            dialog.title(if entry.is_some() { "View / edit cookie" } else { "Add cookie" }).w(px(640.)).confirm()
                .button_props(DialogButtonProps::default().ok_text("Save cookie"))
                .child(v_flex().gap_2().child("Origin URL").child(Input::new(&origin))
                    .child("Set-Cookie value (including Path, Domain, Secure, HttpOnly and expiry attributes)").child(Input::new(&header)))
                .on_ok(move |_, window, cx| {
                    let result = jar.edit(old.as_ref(), origin_ok.read(cx).value().as_ref(), header_ok.read(cx).value().as_ref());
                    if let Err(error) = result { window.push_notification(Notification::error(error), cx); return false; }
                    if let Some(this) = this.upgrade() { this.update(cx, |_, cx| cx.notify()); }
                    true
                })
        });
    }
}

fn cookie_key(entry: &CookieEntry) -> (String, String, String) {
    (entry.name.clone(), entry.domain.clone(), entry.path.clone())
}

fn cookie_table_cell(text: String, cx: &App) -> impl IntoElement {
    div()
        .flex_1()
        .min_w_0()
        .h_full()
        .px_3()
        .border_l_1()
        .border_color(cx.api_outline_variant())
        .flex()
        .items_center()
        .child(div().truncate().child(text))
}
