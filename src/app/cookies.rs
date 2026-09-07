use super::*;
use crate::core::CookieEntry;

impl ApiTester {
    pub(super) fn open_cookie_manager(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let jar = self.cookie_jar.clone();
        let this = cx.entity().downgrade();
        let workspace = self.active_workspace_name().to_owned();
        window.open_dialog(cx, move |dialog, _, cx| {
            let toggle_jar = jar.clone();
            let toggle_this = this.clone();
            let add_this = this.clone();
            let reset_this = this.clone();
            let reset_jar = jar.clone();
            let warning = jar.warning();
            dialog.title(format!("Cookies — {workspace}")).w(px(720.)).child(
                v_flex().gap_3()
                    .child("Cookies belong to this workspace and are stored encrypted. Explicit Cookie headers override the jar.")
                    .when_some(warning, |view, warning| view.child(div().text_color(cx.theme().danger).child(warning)))
                    .child(h_flex().gap_2()
                        .child(Button::new("toggle-cookies").debug_selector(|| "toggle-cookies".to_owned()).label(if jar.enabled() { "Disable automatic cookies" } else { "Enable automatic cookies" })
                            .on_click(move |_, window, cx| {
                                if let Err(error) = toggle_jar.set_enabled(!toggle_jar.enabled()) { window.push_notification(Notification::error(error), cx); }
                                if let Some(this) = toggle_this.upgrade() { this.update(cx, |_, cx| cx.notify()); }
                                window.refresh();
                            }))
                        .child(Button::new("add-cookie").debug_selector(|| "add-cookie".to_owned()).label("Add cookie").on_click(move |_, window, cx| {
                            if let Some(this) = add_this.upgrade() { this.update(cx, |this, cx| this.open_cookie_editor(None, window, cx)); }
                        }))
                        .child(Button::new("clear-cookies").debug_selector(|| "clear-cookies".to_owned()).label("Clear / reset jar").danger().on_click(move |_, window, cx| {
                            let jar = reset_jar.clone(); let this = reset_this.clone();
                            window.open_dialog(cx, move |dialog, _, _| {
                                let jar = jar.clone(); let this = this.clone();
                                dialog.title("Clear this workspace's cookies?").confirm()
                                    .child("This removes all saved cookies, including unreadable storage. Other workspaces are unaffected.")
                                    .on_ok(move |_, window, cx| {
                                        if let Err(error) = jar.clear() { window.push_notification(Notification::error(error), cx); return false; }
                                        if let Some(this) = this.upgrade() { this.update(cx, |_, cx| cx.notify()); }
                                        true
                                    })
                            });
                        })))
                    .child(div().id("cookie-list").max_h(px(380.)).overflow_y_scroll().child(
                        v_flex().gap_2().when(jar.entries().is_empty(), |view| view.child("No cookies in this workspace."))
                            .children(jar.entries().into_iter().enumerate().map(|(index, entry)| {
                                let edit_entry = entry.clone(); let edit_this = this.clone();
                                let delete_jar = jar.clone(); let delete_this = this.clone(); let delete_entry = entry.clone();
                                h_flex().gap_2().child(v_flex().flex_1().min_w_0()
                                    .child(entry.name.clone()).child(div().text_sm().text_color(cx.theme().muted_foreground).child(format!("{}{} · value hidden", entry.domain, entry.path))))
                                    .child(Button::new(("edit-cookie", index)).label("View / edit").on_click(move |_, window, cx| {
                                        if let Some(this) = edit_this.upgrade() { this.update(cx, |this, cx| this.open_cookie_editor(Some(edit_entry.clone()), window, cx)); }
                                    }))
                                    .child(Button::new(("delete-cookie", index)).label("Delete").on_click(move |_, window, cx| {
                                        if let Err(error) = delete_jar.delete(&delete_entry) { window.push_notification(Notification::error(error), cx); }
                                        if let Some(this) = delete_this.upgrade() { this.update(cx, |_, cx| cx.notify()); }
                                        window.refresh();
                                    }))
                            }))
                    ))
            )
        });
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
