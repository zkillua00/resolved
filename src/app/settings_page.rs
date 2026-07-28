use gpui_component::setting::{
    SettingField, SettingGroup, SettingItem, SettingPage, Settings as SettingsView,
};

use super::*;

impl ApiTester {
    pub(super) fn render_settings_title_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let has_overrides = shortcuts::shortcut_descriptors()
            .iter()
            .any(|descriptor| self.settings.shortcuts.contains_key(descriptor.id.key()));

        h_flex()
            .h(px(64.))
            .flex_shrink_0()
            .pl(px(92.))
            .pr_6()
            .border_b_1()
            .border_color(cx.theme().title_bar_border)
            .bg(cx.theme().title_bar)
            .justify_between()
            .child(
                h_flex()
                    .gap_6()
                    .child(
                        div()
                            .text_xl()
                            .font_semibold()
                            .text_color(cx.api_primary_bright())
                            .child("API Tester"),
                    )
                    .child(
                        h_flex()
                            .h_full()
                            .items_center()
                            .border_b_2()
                            .border_color(cx.theme().primary)
                            .px_1()
                            .text_sm()
                            .font_semibold()
                            .child("Settings"),
                    ),
            )
            .child(
                Button::new("reset-all-shortcuts")
                    .label("Reset shortcuts")
                    .outline()
                    .disabled(!has_overrides || !self.settings_writable)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.reset_all_shortcuts(cx);
                    })),
            )
            .into_any_element()
    }

    pub(super) fn render_settings_workspace(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut keyboard_page = SettingPage::new("Keyboard")
            .description("Record shortcuts directly. Defaults follow familiar macOS conventions.")
            .resettable(false);
        for category in [
            shortcuts::ShortcutCategory::Request,
            shortcuts::ShortcutCategory::Navigation,
            shortcuts::ShortcutCategory::View,
            shortcuts::ShortcutCategory::Application,
        ] {
            let items = shortcuts::shortcut_descriptors()
                .iter()
                .filter(|descriptor| descriptor.category == category)
                .map(|descriptor| self.shortcut_setting_item(*descriptor, cx))
                .collect::<Vec<_>>();
            keyboard_page =
                keyboard_page.group(SettingGroup::new().title(category.label()).items(items));
        }

        let appearance_page = SettingPage::new("Appearance")
            .description("Load a CSS theme and map its semantic colors into GPUI.")
            .resettable(false)
            .group(
                SettingGroup::new()
                    .title("Theme")
                    .item(self.theme_setting_item(cx)),
            );

        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            .when_some(self.settings_warning.clone(), |this, warning| {
                this.child(settings_message(warning, cx.theme().danger))
            })
            .when_some(self.settings_notice.clone(), |this, notice| {
                this.child(settings_message(notice, cx.theme().info))
            })
            .child(
                div().flex_1().min_h_0().child(
                    SettingsView::new("api-tester-settings")
                        .sidebar_width(px(220.))
                        .pages([keyboard_page, appearance_page]),
                ),
            )
            .into_any_element()
    }

    fn shortcut_setting_item(
        &self,
        descriptor: shortcuts::ShortcutDescriptor,
        cx: &mut Context<Self>,
    ) -> SettingItem {
        let this = cx.entity().downgrade();
        SettingItem::new(
            descriptor.label,
            SettingField::<SharedString>::render(move |_, _, cx| {
                let Some(entity) = this.upgrade() else {
                    return div().into_any_element();
                };
                let state = entity.read(cx);
                let effective = shortcuts::effective_binding(&state.settings, descriptor.id).ok();
                let binding = effective
                    .as_ref()
                    .and_then(|shortcut| shortcut.binding.as_deref());
                let recording = state.recording_shortcut_id == Some(descriptor.id);
                let customized = state.settings.shortcuts.contains_key(descriptor.id.key());
                let writable = state.settings_writable;

                let record_this = this.clone();
                let clear_this = this.clone();
                let reset_this = this.clone();
                h_flex()
                    .gap_2()
                    .child(
                        Button::new(SharedString::from(format!(
                            "record-shortcut-{}",
                            descriptor.id.key()
                        )))
                        .label(if recording {
                            "Press keys…".to_owned()
                        } else {
                            shortcut_display(binding)
                        })
                        .outline()
                        .disabled(!writable)
                        .on_click(move |_, _, cx| {
                            if let Some(this) = record_this.upgrade() {
                                this.update(cx, |this, cx| {
                                    this.begin_recording_shortcut(descriptor.id, cx);
                                });
                            }
                        }),
                    )
                    .child(
                        Button::new(SharedString::from(format!(
                            "clear-shortcut-{}",
                            descriptor.id.key()
                        )))
                        .label("Clear")
                        .ghost()
                        .disabled(!writable || binding.is_none())
                        .on_click(move |_, _, cx| {
                            if let Some(this) = clear_this.upgrade() {
                                this.update(cx, |this, cx| {
                                    this.clear_shortcut(descriptor.id, cx);
                                });
                            }
                        }),
                    )
                    .when(customized, |row| {
                        row.child(
                            Button::new(SharedString::from(format!(
                                "reset-shortcut-{}",
                                descriptor.id.key()
                            )))
                            .label("Reset")
                            .ghost()
                            .disabled(!writable)
                            .on_click(move |_, _, cx| {
                                if let Some(this) = reset_this.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.reset_shortcut(descriptor.id, cx);
                                    });
                                }
                            }),
                        )
                    })
                    .into_any_element()
            }),
        )
        .description(format!(
            "{} · default {}",
            descriptor.id.key(),
            shortcut_display(Some(descriptor.default_binding))
        ))
    }

    fn theme_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let this = cx.entity().downgrade();
        SettingItem::new(
            "CSS theme",
            SettingField::<SharedString>::render(move |_, _, cx| {
                let Some(entity) = this.upgrade() else {
                    return div().into_any_element();
                };
                let state = entity.read(cx);
                let path = state
                    .settings
                    .theme
                    .source_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "Built-in Material Dark".to_owned());
                let has_custom_theme = state.settings.theme.css_source.is_some();
                let can_reload = state.settings.theme.source_path.is_some();
                let writable = state.settings_writable;

                let choose_this = this.clone();
                let reload_this = this.clone();
                let reset_this = this.clone();
                v_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        div()
                            .max_w(px(520.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(path),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("choose-css-theme")
                                    .label("Choose CSS…")
                                    .outline()
                                    .disabled(!writable)
                                    .on_click(move |_, window, cx| {
                                        if let Some(this) = choose_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.choose_css_theme(window, cx);
                                            });
                                        }
                                    }),
                            )
                            .child(
                                Button::new("reload-css-theme")
                                    .label("Reload")
                                    .ghost()
                                    .disabled(!writable || !can_reload)
                                    .on_click(move |_, _, cx| {
                                        if let Some(this) = reload_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.reload_css_theme(cx);
                                            });
                                        }
                                    }),
                            )
                            .when(has_custom_theme, |row| {
                                row.child(
                                    Button::new("reset-css-theme")
                                        .label("Use built-in")
                                        .ghost()
                                        .disabled(!writable)
                                        .on_click(move |_, _, cx| {
                                            if let Some(this) = reset_this.upgrade() {
                                                this.update(cx, |this, cx| {
                                                    this.reset_css_theme(cx);
                                                });
                                            }
                                        }),
                                )
                            }),
                    )
                    .into_any_element()
            }),
        )
        .description(
            "Imports one :root stylesheet, validates semantic --api-* variables, and keeps a durable snapshot for restart.",
        )
    }
}

fn shortcut_display(binding: Option<&str>) -> String {
    let Some(binding) = binding else {
        return "Unassigned".to_owned();
    };
    binding
        .split_whitespace()
        .map(|stroke| {
            stroke
                .split('-')
                .map(|part| match part {
                    "cmd" => "⌘".to_owned(),
                    "ctrl" => "⌃".to_owned(),
                    "alt" => "⌥".to_owned(),
                    "shift" => "⇧".to_owned(),
                    "enter" => "↩".to_owned(),
                    "tab" => "⇥".to_owned(),
                    "\\" => "\\".to_owned(),
                    key => key.to_uppercase(),
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn settings_message(message: String, color: Hsla) -> AnyElement {
    div()
        .mx_4()
        .mt_3()
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(color.opacity(0.45))
        .bg(color.opacity(0.1))
        .text_sm()
        .text_color(color)
        .child(message)
        .into_any_element()
}
