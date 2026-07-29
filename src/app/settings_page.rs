use gpui_component::group_box::GroupBoxVariant;
use gpui_component::setting::{
    SettingField, SettingGroup, SettingItem, SettingPage, Settings as SettingsView,
};

use super::*;

impl ApiTester {
    pub(super) fn render_settings_title_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .h(px(APP_TITLE_BAR_HEIGHT))
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
            .into_any_element()
    }

    pub(super) fn render_settings_workspace(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut keyboard_page = SettingPage::new("Keyboard")
            .description("Record shortcuts directly. Defaults follow familiar macOS conventions.")
            .default_open(true)
            .resettable(false);
        for category in shortcuts::ShortcutCategory::ALL {
            let mut items = shortcuts::shortcut_descriptors()
                .iter()
                .filter(|descriptor| descriptor.category == category)
                .map(|descriptor| self.shortcut_setting_item(*descriptor, cx))
                .collect::<Vec<_>>();
            if category == shortcuts::ShortcutCategory::Application {
                items.push(self.reset_all_shortcuts_setting_item(cx));
            }
            keyboard_page = keyboard_page.group(
                SettingGroup::new()
                    .title(category.label())
                    .description(category.description())
                    .items(items),
            );
        }

        let appearance_page = SettingPage::new("Appearance")
            .description(
                "Choose a built-in or saved theme, then edit its validated CSS here or in your preferred editor.",
            )
            .resettable(false)
            .group(
                SettingGroup::new()
                    .title("Themes")
                    .description(
                        "Saved themes remain available when you switch. Invalid drafts never replace the active theme.",
                    )
                    .item(self.theme_setting_item(cx)),
            );
        let developer_page = SettingPage::new("Developer Settings")
            .description("Enable diagnostics for inspecting API Tester while it is running.")
            .resettable(false)
            .group(
                SettingGroup::new()
                    .title("Diagnostics")
                    .description("Developer overlays stay inactive until explicitly enabled.")
                    .item(self.metrics_setting_item(cx)),
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
                        .with_group_variant(GroupBoxVariant::Outline)
                        .pages([keyboard_page, appearance_page, developer_page]),
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
                    .w(px(330.))
                    .justify_end()
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
                        .w(px(132.))
                        .disabled(!writable)
                        .tooltip(format!("Record shortcut for {}", descriptor.label))
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
                        .w(px(72.))
                        .disabled(!writable || binding.is_none())
                        .tooltip(format!("Clear shortcut for {}", descriptor.label))
                        .on_click(move |_, _, cx| {
                            if let Some(this) = clear_this.upgrade() {
                                this.update(cx, |this, cx| {
                                    this.clear_shortcut(descriptor.id, cx);
                                });
                            }
                        }),
                    )
                    .child(
                        Button::new(SharedString::from(format!(
                            "reset-shortcut-{}",
                            descriptor.id.key()
                        )))
                        .label("Reset")
                        .ghost()
                        .w(px(72.))
                        .disabled(!writable || !customized)
                        .tooltip(format!("Restore the default for {}", descriptor.label))
                        .on_click(move |_, _, cx| {
                            if let Some(this) = reset_this.upgrade() {
                                this.update(cx, |this, cx| {
                                    this.reset_shortcut(descriptor.id, cx);
                                });
                            }
                        }),
                    )
                    .into_any_element()
            }),
        )
        .description(format!(
            "Default: {}",
            shortcut_display(Some(descriptor.default_binding))
        ))
    }

    fn reset_all_shortcuts_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let this = cx.entity().downgrade();
        SettingItem::new(
            "Shortcut defaults",
            SettingField::<SharedString>::render(move |_, _, cx| {
                let Some(entity) = this.upgrade() else {
                    return div().into_any_element();
                };
                let state = entity.read(cx);
                let has_overrides = shortcuts::shortcut_descriptors()
                    .iter()
                    .any(|descriptor| state.settings.shortcuts.contains_key(descriptor.id.key()));
                let reset_this = this.clone();
                Button::new("reset-all-shortcuts")
                    .label("Reset all")
                    .outline()
                    .disabled(!has_overrides || !state.settings_writable)
                    .tooltip("Restore every shortcut to its default binding")
                    .on_click(move |_, _, cx| {
                        if let Some(this) = reset_this.upgrade() {
                            this.update(cx, |this, cx| {
                                this.reset_all_shortcuts(cx);
                            });
                        }
                    })
                    .into_any_element()
            }),
        )
        .description("Restore every keyboard command to its default binding.")
    }

    fn metrics_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let this = cx.entity().downgrade();
        let read_this = this.clone();
        SettingItem::new(
            "Metrics",
            SettingField::<bool>::switch(
                move |cx| {
                    read_this.upgrade().is_some_and(|entity| {
                        entity.read(cx).debug_overlay.read(cx).is_visible()
                    })
                },
                move |visible, cx| {
                    if let Some(entity) = this.upgrade() {
                        entity.update(cx, |this, cx| {
                            this.debug_overlay.update(cx, |overlay, cx| {
                                overlay.set_visible(visible, cx);
                            });
                            cx.notify();
                        });
                    }
                },
            ),
        )
        .description(
            "Show the performance HUD with UI redraw cadence, process CPU, RSS, and physical footprint.",
        )
    }

    fn theme_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let this = cx.entity().downgrade();
        SettingItem::new(
            "Current theme",
            SettingField::<SharedString>::render(move |_, _, cx| {
                let Some(entity) = this.upgrade() else {
                    return div().into_any_element();
                };
                let state = entity.read(cx);
                let active_theme_id = state.settings.theme.active_theme_id.clone();
                let detached_theme = state.has_detached_theme_snapshot();
                let detached_theme_name = detached_theme.then(|| {
                    state
                        .settings
                        .theme
                        .css_source
                        .as_deref()
                        .and_then(|source| crate::theme::parse_css(source).ok())
                        .map(|theme| theme.name.to_string())
                        .unwrap_or_else(|| "Unsaved CSS theme".to_owned())
                });
                let active_saved_theme = active_theme_id.as_ref().and_then(|active_id| {
                    state
                        .settings
                        .theme
                        .saved_themes
                        .iter()
                        .find(|theme| theme.id == *active_id)
                });
                let active_theme_name =
                    match (active_theme_id.as_ref(), active_saved_theme, detached_theme_name) {
                    (_, _, Some(name)) => format!("{name} · unsaved"),
                    (Some(_), _, None) if detached_theme => {
                        "Unreconciled theme snapshot".to_owned()
                    }
                    (None, _, None) => "API Tester Material Dark".to_owned(),
                    (Some(_), Some(theme), _) => theme.name.clone(),
                    (Some(_), None, _) => "Unavailable saved theme".to_owned(),
                };
                let built_in_active =
                    active_theme_id.is_none() && state.settings.theme.css_source.is_none();
                let path = state
                    .settings
                    .theme
                    .draft_path
                    .as_ref()
                    .or_else(|| {
                        (!detached_theme)
                            .then(|| active_saved_theme.and_then(|theme| theme.source_path.as_ref()))
                            .flatten()
                    })
                    .or(state.settings.theme.source_path.as_ref())
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| {
                        if detached_theme {
                            "Unreconciled CSS snapshot · no source file".to_owned()
                        } else if active_saved_theme.is_some() {
                            "Saved SQLite snapshot · source file created on demand".to_owned()
                        } else {
                            "Built-in theme · no source file".to_owned()
                        }
                    });
                let can_reload = state.settings.theme.source_path.is_some()
                    || state.settings.theme.draft_path.is_some();
                let writable = state.settings_writable;
                let editor_open = state.theme_editor.is_some();
                let editor_pending = state.has_unapplied_editor_draft();
                let theme_pending = state.has_unapplied_theme_draft();
                let external_draft_pending =
                    state.settings.theme.draft_path.is_some() && !editor_pending;
                let selection_blocked = theme_pending || detached_theme;
                let danger = cx.theme().danger;

                let switch_this = this.clone();
                let choose_this = this.clone();
                let edit_this = this.clone();
                let external_this = this.clone();
                let reload_this = this.clone();
                let discard_external_this = this.clone();
                let delete_this = this.clone();
                let menu_themes = state
                    .settings
                    .theme
                    .saved_themes
                    .iter()
                    .map(|theme| (theme.id.clone(), theme.name.clone()))
                    .collect::<Vec<_>>();
                let menu_active_theme_id = active_theme_id.clone();
                let menu_built_in_active = built_in_active;
                v_flex()
                    .w_full()
                    .gap_3()
                    .child(
                        Button::new("css-theme-picker")
                            .label(active_theme_name)
                            .icon(IconName::Palette)
                            .dropdown_caret(true)
                            .outline()
                            .w(px(400.))
                            .disabled(!writable || selection_blocked)
                            .tooltip(if detached_theme {
                                "Save this unsaved CSS snapshot as a new theme before switching"
                            } else if external_draft_pending {
                                "Reload or discard the preferred-editor copy before switching themes"
                            } else if editor_pending {
                                "Save or revert the CSS draft before switching themes"
                            } else {
                                "Switch between the built-in theme and saved CSS themes"
                            })
                            .dropdown_menu_with_anchor(Corner::TopRight, move |menu, _, _| {
                                let built_in_this = switch_this.clone();
                                let mut menu = menu
                                    .min_w(px(400.))
                                    .max_h(px(360.))
                                    .scrollable(menu_themes.len() > 8)
                                    .label("Built-in")
                                    .item(
                                        PopupMenuItem::new("API Tester Material Dark")
                                            .icon(IconName::Palette)
                                            .checked(menu_built_in_active)
                                            .on_click(move |_, _, cx| {
                                                if menu_built_in_active {
                                                    return;
                                                }
                                                if let Some(this) = built_in_this.upgrade() {
                                                    this.update(cx, |this, cx| {
                                                        this.switch_css_theme(None, cx);
                                                    });
                                                }
                                            }),
                                    )
                                    .separator()
                                    .label("Saved themes");
                                if menu_themes.is_empty() {
                                    menu = menu.item(
                                        PopupMenuItem::new("No saved themes yet").disabled(true),
                                    );
                                } else {
                                    for (theme_id, theme_name) in &menu_themes {
                                        let theme_id = theme_id.clone();
                                        let item_this = switch_this.clone();
                                        let selected =
                                            menu_active_theme_id.as_ref() == Some(&theme_id);
                                        menu = menu.item(
                                            PopupMenuItem::new(theme_name.clone())
                                                .checked(selected)
                                                .on_click(move |_, _, cx| {
                                                    if selected {
                                                        return;
                                                    }
                                                    if let Some(this) = item_this.upgrade() {
                                                        this.update(cx, |this, cx| {
                                                            this.switch_css_theme(
                                                                Some(theme_id.clone()),
                                                                cx,
                                                            );
                                                        });
                                                    }
                                                }),
                                        );
                                    }
                                }
                                menu
                            }),
                    )
                    .child(
                        div()
                            .id("css-theme-source-path")
                            .max_w(px(520.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .tooltip({
                                let path = path.clone();
                                move |window, cx| Tooltip::new(path.clone()).build(window, cx)
                            })
                            .child(path),
                    )
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                Button::new("choose-css-theme")
                                    .label("Import CSS…")
                                    .outline()
                                    .disabled(!writable || selection_blocked)
                                    .tooltip("Import and apply a CSS theme file")
                                    .on_click(move |_, window, cx| {
                                        if let Some(this) = choose_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.choose_css_theme(window, cx);
                                            });
                                        }
                                    }),
                            )
                            .child(
                                Button::new("edit-css-theme-here")
                                    .label(if editor_open {
                                        "Show CSS editor"
                                    } else {
                                        "Edit CSS here"
                                    })
                                    .outline()
                                    .disabled(!writable)
                                    .tooltip(if editor_open {
                                        "Show the open Theme CSS workspace tab"
                                    } else {
                                        "Open a Theme CSS workspace tab with validation and intelligence"
                                    })
                                    .on_click(move |_, window, cx| {
                                        if let Some(this) = edit_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.open_theme_editor(window, cx);
                                            });
                                        }
                                    }),
                            )
                            .child(
                                Button::new("open-css-theme-preferred-editor")
                                    .label("Open in preferred editor")
                                    .ghost()
                                    .disabled(!writable)
                                    .tooltip("Use the macOS default application for CSS files")
                                    .on_click(move |_, _, cx| {
                                        if let Some(this) = external_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.open_css_in_preferred_editor(cx);
                                            });
                                        }
                                    }),
                            )
                            .child(
                                Button::new("reload-css-theme")
                                    .label("Reload")
                                    .ghost()
                                    .disabled(
                                        !writable
                                            || !can_reload
                                            || editor_pending
                                            || detached_theme,
                                    )
                                    .on_click(move |_, _, cx| {
                                        if let Some(this) = reload_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.reload_css_theme(cx);
                                            });
                                        }
                                    }),
                            )
                            .when(external_draft_pending, |row| {
                                row.child(
                                    Button::new("discard-external-css-draft")
                                        .label("Discard external copy")
                                        .ghost()
                                        .disabled(!writable)
                                        .tooltip(
                                            "Stop tracking the preferred-editor copy without deleting its file",
                                        )
                                        .on_click(move |_, _, cx| {
                                            if let Some(this) = discard_external_this.upgrade() {
                                                this.update(cx, |this, cx| {
                                                    this.discard_external_theme_draft(cx);
                                                });
                                            }
                                        }),
                                )
                            })
                            .when(active_theme_id.is_some(), |row| {
                                row.child(
                                    Button::new("delete-selected-css-theme")
                                        .label("Delete selected theme…")
                                        .ghost()
                                        .text_color(danger)
                                        .disabled(!writable || selection_blocked)
                                        .tooltip("Delete this saved theme after confirmation")
                                        .on_click(move |_, window, cx| {
                                            if let Some(this) = delete_this.upgrade() {
                                                this.update(cx, |this, cx| {
                                                    this.delete_active_saved_theme(window, cx);
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
            "The built-in theme is read-only. Import a CSS file or use the editor to save another theme, then switch here without losing it.",
        )
        .layout(Axis::Vertical)
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

pub(super) fn settings_message(message: String, color: Hsla) -> AnyElement {
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
