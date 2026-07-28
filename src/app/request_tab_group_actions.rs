use super::*;

const MAX_TAB_GROUP_TITLE_CHARS: usize = 80;

impl ApiTester {
    pub(super) fn expand_request_tab_group_for(&mut self, tab_id: &RequestTabId) -> bool {
        let Some(group_id) = self
            .request_tabs
            .get(tab_id)
            .and_then(RequestTabRecord::group_id)
            .cloned()
        else {
            return false;
        };
        self.request_tabs.set_group_collapsed(&group_id, false)
    }

    pub(super) fn open_new_request_tab_group_dialog(
        &mut self,
        tab_id: RequestTabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.request_tabs.get(&tab_id).is_none() {
            return;
        }
        let default_title = self.next_request_tab_group_title();
        let title_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Group name")
                .default_value(default_title)
        });
        let this = cx.entity().downgrade();
        let input_for_dialog = title_input.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let this_for_ok = this.clone();
            let input_for_ok = input_for_dialog.clone();
            let tab_for_ok = tab_id.clone();
            dialog
                .title("Create tab group")
                .w(px(440.))
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("Create group"))
                .on_ok(move |_, _, cx| {
                    let Some(title) = normalized_tab_group_title(&input_for_ok.read(cx).value())
                    else {
                        return false;
                    };
                    let Some(this) = this_for_ok.upgrade() else {
                        return true;
                    };
                    this.update(cx, |this, cx| {
                        this.create_request_tab_group(tab_for_ok.clone(), title, cx);
                    });
                    true
                })
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("Name the group for this request tab."),
                        )
                        .child(Input::new(&input_for_dialog)),
                )
        });
        title_input.read(cx).focus_handle(cx).focus(window);
    }

    pub(super) fn open_rename_request_tab_group_dialog(
        &mut self,
        group_id: RequestTabGroupId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(current_title) = self
            .request_tabs
            .group(&group_id)
            .map(|group| group.display_title().to_owned())
        else {
            return;
        };
        let title_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Group name")
                .default_value(current_title)
        });
        let this = cx.entity().downgrade();
        let input_for_dialog = title_input.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let this_for_ok = this.clone();
            let input_for_ok = input_for_dialog.clone();
            let group_for_ok = group_id.clone();
            dialog
                .title("Rename tab group")
                .w(px(440.))
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("Rename"))
                .on_ok(move |_, _, cx| {
                    let Some(title) = normalized_tab_group_title(&input_for_ok.read(cx).value())
                    else {
                        return false;
                    };
                    let Some(this) = this_for_ok.upgrade() else {
                        return true;
                    };
                    this.update(cx, |this, cx| {
                        this.rename_request_tab_group(group_for_ok.clone(), title, cx);
                    });
                    true
                })
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("Enter a new name for this tab group."),
                        )
                        .child(Input::new(&input_for_dialog)),
                )
        });
        title_input.read(cx).focus_handle(cx).focus(window);
    }

    pub(super) fn create_request_tab_group(
        &mut self,
        tab_id: RequestTabId,
        title: String,
        cx: &mut Context<Self>,
    ) {
        let color = self.next_request_tab_group_color();
        let Some(group_id) = self
            .request_tabs
            .create_group_for_tab(&tab_id, title, color)
        else {
            return;
        };
        let _ = self.request_tabs.set_group_collapsed(&group_id, false);
        self.persist_request_tabs_now(cx);
        cx.notify();
    }

    pub(super) fn assign_request_tab_to_group(
        &mut self,
        tab_id: RequestTabId,
        group_id: RequestTabGroupId,
        cx: &mut Context<Self>,
    ) {
        if !self.request_tabs.set_tab_group(&tab_id, Some(&group_id)) {
            return;
        }
        let _ = self.request_tabs.set_group_collapsed(&group_id, false);
        self.persist_request_tabs_now(cx);
        cx.notify();
    }

    pub(super) fn remove_request_tab_from_group(
        &mut self,
        tab_id: RequestTabId,
        cx: &mut Context<Self>,
    ) {
        if !self.request_tabs.set_tab_group(&tab_id, None) {
            return;
        }
        self.persist_request_tabs_now(cx);
        cx.notify();
    }

    pub(super) fn rename_request_tab_group(
        &mut self,
        group_id: RequestTabGroupId,
        title: String,
        cx: &mut Context<Self>,
    ) {
        if !self.request_tabs.rename_group(&group_id, title) {
            return;
        }
        self.persist_request_tabs_now(cx);
        cx.notify();
    }

    pub(super) fn set_request_tab_group_color(
        &mut self,
        group_id: RequestTabGroupId,
        color: RequestTabGroupColor,
        cx: &mut Context<Self>,
    ) {
        if !self.request_tabs.set_group_color(&group_id, color) {
            return;
        }
        self.persist_request_tabs_now(cx);
        cx.notify();
    }

    pub(super) fn toggle_request_tab_group_collapsed(
        &mut self,
        group_id: RequestTabGroupId,
        cx: &mut Context<Self>,
    ) {
        let Some(collapsed) = self
            .request_tabs
            .group(&group_id)
            .map(RequestTabGroup::is_collapsed)
        else {
            return;
        };
        if !self.request_tabs.set_group_collapsed(&group_id, !collapsed) {
            return;
        }
        self.persist_request_tabs_now(cx);
        cx.notify();
    }

    pub(super) fn remove_request_tab_group(
        &mut self,
        group_id: RequestTabGroupId,
        cx: &mut Context<Self>,
    ) {
        if !self.request_tabs.remove_group(&group_id) {
            return;
        }
        self.persist_request_tabs_now(cx);
        cx.notify();
    }

    pub(super) fn open_blank_request_tab_in_group(
        &mut self,
        group_id: RequestTabGroupId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sending {
            return;
        }
        if self.request_tabs.group(&group_id).is_none() {
            return;
        }
        self.snapshot_active_request_tab(cx);
        let _ = self.request_tabs.set_group_collapsed(&group_id, false);
        let tab_id = if self.selected_collection_id.is_none() {
            self.request_tabs
                .open_new_in_group(&group_id)
                .expect("the tab group was validated before opening")
        } else {
            let tab_id = self.request_tabs.open_unsaved(
                DEFAULT_REQUEST_TAB_TITLE,
                RequestTemplate::default(),
                RequestTabAssociation::new(
                    self.selected_folder_id.clone(),
                    self.selected_collection_id.clone(),
                    None,
                ),
            );
            let grouped = self.request_tabs.set_tab_group(&tab_id, Some(&group_id));
            debug_assert!(grouped, "the validated group must accept a new tab");
            tab_id
        };
        self.request_tab_runtime
            .insert(tab_id.as_str().to_owned(), RequestTabRuntime::default());
        self.hide_preview(cx);
        self.restore_active_request_tab(window, cx);
        self.persist_request_tabs_now(cx);
    }

    fn next_request_tab_group_title(&self) -> String {
        let mut index = self.request_tabs.groups().len() + 1;
        loop {
            let candidate = format!("Group {index}");
            if self
                .request_tabs
                .groups()
                .iter()
                .all(|group| group.display_title() != candidate)
            {
                return candidate;
            }
            index += 1;
        }
    }

    fn next_request_tab_group_color(&self) -> RequestTabGroupColor {
        match self.request_tabs.groups().len() % 8 {
            0 => RequestTabGroupColor::Purple,
            1 => RequestTabGroupColor::Blue,
            2 => RequestTabGroupColor::Cyan,
            3 => RequestTabGroupColor::Green,
            4 => RequestTabGroupColor::Yellow,
            5 => RequestTabGroupColor::Orange,
            6 => RequestTabGroupColor::Pink,
            _ => RequestTabGroupColor::Red,
        }
    }
}

fn normalized_tab_group_title(value: &str) -> Option<String> {
    let title = value.trim();
    if title.is_empty() {
        return None;
    }
    Some(title.chars().take(MAX_TAB_GROUP_TITLE_CHARS).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_group_titles_are_trimmed_bounded_and_non_empty() {
        assert_eq!(
            normalized_tab_group_title("  Integration  "),
            Some("Integration".to_owned())
        );
        assert_eq!(normalized_tab_group_title("  "), None);
        let long = "x".repeat(MAX_TAB_GROUP_TITLE_CHARS + 12);
        assert_eq!(
            normalized_tab_group_title(&long)
                .expect("long names are truncated")
                .chars()
                .count(),
            MAX_TAB_GROUP_TITLE_CHARS
        );
    }
}
