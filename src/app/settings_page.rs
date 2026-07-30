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

        let appearance_page = SettingPage::new("Appearance").resettable(false).group(
            SettingGroup::new()
                .with_variant(GroupBoxVariant::Normal)
                .items([
                    self.theme_global_actions_setting_item(cx),
                    self.theme_library_setting_item(cx),
                ]),
        );
        let developer_page = SettingPage::new("Developer Settings")
            .description("Enable diagnostics for inspecting API Tester while it is running.")
            .resettable(false)
            .group(
                SettingGroup::new()
                    .title("Diagnostics")
                    .description("Developer overlays stay inactive until explicitly enabled.")
                    .items([
                        self.metrics_setting_item(cx),
                        self.metrics_position_setting_item(cx),
                    ]),
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

    fn metrics_position_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let this = cx.entity().downgrade();
        SettingItem::new(
            "Metrics location",
            SettingField::<SharedString>::render(move |_, _, cx| {
                let Some(entity) = this.upgrade() else {
                    return div().into_any_element();
                };
                let state = entity.read(cx);
                let selected = state.settings.metrics_position;
                let writable = state.settings_writable;
                let menu_this = this.clone();

                Button::new("metrics-position-picker")
                    .label(selected.label())
                    .dropdown_caret(true)
                    .outline()
                    .w(px(220.))
                    .disabled(!writable)
                    .tooltip(if writable {
                        "Choose which workspace corner contains the Metrics HUD"
                    } else {
                        "Settings are read-only because they could not be loaded safely"
                    })
                    .dropdown_menu(move |mut menu, _, _| {
                        menu = menu.min_w(px(220.));
                        for position in crate::core::MetricsPosition::ALL {
                            let item_this = menu_this.clone();
                            menu = menu.item(
                                PopupMenuItem::new(position.label())
                                    .checked(position == selected)
                                    .on_click(move |_, _, cx| {
                                        if position == selected {
                                            return;
                                        }
                                        if let Some(this) = item_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.set_metrics_position(position, cx);
                                            });
                                        }
                                    }),
                            );
                        }
                        menu
                    })
                    .into_any_element()
            }),
        )
        .description("Choose the HUD corner. The location is restored when API Tester restarts.")
    }

    fn theme_global_actions_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let this = cx.entity().downgrade();
        SettingItem::new(
            "Themes",
            SettingField::<SharedString>::render(move |_, _, cx| {
                let Some(entity) = this.upgrade() else {
                    return div().into_any_element();
                };
                let state = entity.read(cx);
                let detached_theme = state.has_detached_theme_snapshot();
                let can_reload = state.settings.theme.source_path.is_some()
                    || state.settings.theme.draft_path.is_some();
                let writable = state.settings_writable;
                let editor_pending = state.has_unapplied_editor_draft();
                let theme_pending = state.has_unapplied_theme_draft();
                let external_draft_pending =
                    state.settings.theme.draft_path.is_some() && !editor_pending;
                let selection_blocked = theme_pending || detached_theme;
                let active_id_ambiguous = state
                    .settings
                    .theme
                    .active_theme_id
                    .as_deref()
                    .is_some_and(|active_id| {
                        active_id.trim().is_empty()
                            || state
                                .settings
                                .theme
                                .saved_themes
                                .iter()
                                .filter(|theme| theme.id == active_id)
                                .count()
                                != 1
                    });

                let choose_this = this.clone();
                let create_this = this.clone();
                let reload_this = this.clone();
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("choose-css-theme")
                            .label("Import CSS…")
                            .small()
                            .outline()
                            .disabled(!writable || selection_blocked)
                            .tooltip(if detached_theme {
                                "Save this theme before importing another"
                            } else if external_draft_pending {
                                "Reload or ignore the file changes before importing"
                            } else if editor_pending {
                                "Save or revert your changes before importing"
                            } else {
                                "Import a theme from a CSS file"
                            })
                            .on_click(move |_, window, cx| {
                                if let Some(this) = choose_this.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.choose_css_theme(window, cx);
                                    });
                                }
                            }),
                    )
                    .child(
                        Button::new("create-css-theme-from-template")
                            .label("Create from template…")
                            .icon(IconName::Plus)
                            .small()
                            .outline()
                            .disabled(!writable || selection_blocked)
                            .tooltip(if detached_theme {
                                "Save this theme before creating another"
                            } else if external_draft_pending {
                                "Reload or ignore the file changes before creating a theme"
                            } else if editor_pending {
                                "Save or revert your changes before creating a theme"
                            } else {
                                "Create a new theme from the default template"
                            })
                            .on_click(move |_, window, cx| {
                                if let Some(this) = create_this.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.open_create_theme_from_template_dialog(window, cx);
                                    });
                                }
                            }),
                    )
                    .child(
                        Button::new("reload-css-theme")
                            .label("Reload active")
                            .small()
                            .ghost()
                            .disabled(
                                !writable
                                    || !can_reload
                                    || editor_pending
                                    || detached_theme
                                    || active_id_ambiguous,
                            )
                            .tooltip(if !can_reload {
                                "The active theme has no file to reload"
                            } else if active_id_ambiguous {
                                "The active theme cannot be reloaded safely"
                            } else if editor_pending {
                                "Save or revert your changes before reloading"
                            } else if detached_theme {
                                "Save this theme before reloading"
                            } else {
                                "Reload the active theme from its file"
                            })
                            .on_click(move |_, _, cx| {
                                if let Some(this) = reload_this.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.reload_css_theme(cx);
                                    });
                                }
                            }),
                    )
                    .into_any_element()
            }),
        )
    }

    fn theme_library_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let this = cx.entity().downgrade();
        let mut search_text =
            "themes built in saved active available invalid use edit preferred delete source"
                .to_owned();
        for theme in &self.settings.theme.saved_themes {
            search_text.push(' ');
            search_text.push_str(&theme.name);
            if let Some(path) = &theme.source_path {
                search_text.push(' ');
                search_text.push_str(&path.display().to_string());
            }
        }

        SettingItem::render_searchable(search_text, move |_, _, cx| {
            let Some(entity) = this.upgrade() else {
                return div().into_any_element();
            };
            let state = entity.read(cx);
            let active_theme_id = state.settings.theme.active_theme_id.clone();
            let detached_theme = state.has_detached_theme_snapshot();
            let built_in_active = !detached_theme
                && active_theme_id.is_none()
                && state.settings.theme.css_source.is_none();
            let writable = state.settings_writable;
            let editor_open = state.theme_editor.is_some();
            let editor_pending = state.has_unapplied_editor_draft();
            let theme_pending = state.has_unapplied_theme_draft();
            let external_draft_pending =
                state.settings.theme.draft_path.is_some() && !editor_pending;
            let selection_blocked = theme_pending || detached_theme;
            let mut rows = Vec::with_capacity(
                1 + usize::from(detached_theme) + state.settings.theme.saved_themes.len(),
            );

            let switch_built_in_this = this.clone();
            let discard_built_in_this = this.clone();
            let built_in_status = if built_in_active {
                active_theme_badge(cx)
            } else {
                available_theme_badge(cx)
            };
            let built_in_actions = h_flex()
                .w_full()
                .justify_end()
                .gap_1()
                .when(!built_in_active, |actions| {
                    actions.child(
                        Button::new("use-built-in-css-theme")
                            .label("Use theme")
                            .small()
                            .outline()
                            .disabled(!writable || selection_blocked)
                            .tooltip(theme_selection_tooltip(
                                detached_theme,
                                editor_pending,
                                external_draft_pending,
                                "Switch to the built-in theme",
                            ))
                            .on_click(move |_, _, cx| {
                                if let Some(this) = switch_built_in_this.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.switch_css_theme(None, cx);
                                    });
                                }
                            }),
                    )
                })
                .when(built_in_active && external_draft_pending, |actions| {
                    actions.child(
                        Button::new("discard-built-in-external-css-draft")
                            .label("Ignore file changes")
                            .small()
                            .ghost()
                            .disabled(!writable)
                            .tooltip("Keep the file, but stop watching it for changes")
                            .on_click(move |_, _, cx| {
                                if let Some(this) = discard_built_in_this.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.discard_external_theme_draft(cx);
                                    });
                                }
                            }),
                    )
                })
                .into_any_element();
            rows.push(theme_table_row(
                "built-in",
                "API Tester Material Dark".to_owned(),
                "Built in · read-only".to_owned(),
                built_in_status,
                built_in_actions,
                cx,
            ));

            if detached_theme {
                let detached_name = state
                    .settings
                    .theme
                    .css_source
                    .as_deref()
                    .and_then(|source| crate::theme::parse_css(source).ok())
                    .map(|theme| theme.name.to_string())
                    .unwrap_or_else(|| "Unsaved theme".to_owned());
                let detached_source = state
                    .settings
                    .theme
                    .draft_path
                    .as_ref()
                    .or(state.settings.theme.source_path.as_ref())
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "API Tester".to_owned());
                let edit_this = this.clone();
                let external_this = this.clone();
                let discard_this = this.clone();
                let detached_actions = h_flex()
                    .w_full()
                    .justify_end()
                    .gap_1()
                    .child(
                        Button::new("edit-detached-css-theme-here")
                            .label(if editor_open {
                                "Show editor"
                            } else {
                                "Edit here"
                            })
                            .small()
                            .outline()
                            .disabled(!writable)
                            .on_click(move |_, window, cx| {
                                if let Some(this) = edit_this.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.open_theme_editor(window, cx);
                                    });
                                }
                            }),
                    )
                    .child(
                        Button::new("edit-detached-css-theme-externally")
                            .label("Edit in preferred editor")
                            .small()
                            .ghost()
                            .disabled(!writable)
                            .on_click(move |_, _, cx| {
                                if let Some(this) = external_this.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.open_css_in_preferred_editor(cx);
                                    });
                                }
                            }),
                    )
                    .when(external_draft_pending, |actions| {
                        actions.child(
                            Button::new("discard-detached-external-css-draft")
                                .label("Ignore file changes")
                                .small()
                                .ghost()
                                .disabled(!writable)
                                .tooltip("Keep the file, but stop watching it for changes")
                                .on_click(move |_, _, cx| {
                                    if let Some(this) = discard_this.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.discard_external_theme_draft(cx);
                                        });
                                    }
                                }),
                        )
                    })
                    .into_any_element();
                rows.push(theme_table_row(
                    "unsaved",
                    detached_name,
                    detached_source,
                    active_theme_badge(cx),
                    detached_actions,
                    cx,
                ));
            }

            for (index, theme) in state.settings.theme.saved_themes.iter().enumerate() {
                let theme_id = theme.id.clone();
                let ambiguous_id = theme_id.trim().is_empty()
                    || state
                        .settings
                        .theme
                        .saved_themes
                        .iter()
                        .filter(|candidate| candidate.id == theme_id)
                        .count()
                        != 1;
                let projected = !detached_theme
                    && active_theme_id.as_ref() == Some(&theme_id)
                    && state.settings.theme.css_source.as_deref()
                        == Some(theme.css_source.as_str())
                    && state.settings.theme.source_path == theme.source_path;
                let valid = crate::theme::parse_css(&theme.css_source).is_ok();
                let active = projected && valid && !ambiguous_id;
                let source = projected
                    .then_some(state.settings.theme.draft_path.as_ref())
                    .flatten()
                    .map(|path| path.display().to_string())
                    .or_else(|| {
                        theme
                            .source_path
                            .as_ref()
                            .map(|path| path.display().to_string())
                    })
                    .unwrap_or_else(|| "API Tester".to_owned());
                let row_key = if ambiguous_id {
                    format!("ambiguous-{index}-{theme_id}")
                } else {
                    format!("saved-{theme_id}")
                };
                let use_element_id: SharedString = format!("use-saved-css-theme-{row_key}").into();
                let edit_element_id: SharedString =
                    format!("edit-saved-css-theme-here-{row_key}").into();
                let external_element_id: SharedString =
                    format!("edit-saved-css-theme-externally-{row_key}").into();
                let discard_external_element_id: SharedString =
                    format!("discard-saved-css-theme-external-draft-{row_key}").into();
                let delete_element_id: SharedString =
                    format!("delete-saved-css-theme-{row_key}").into();
                let identity_tooltip = "This theme’s saved information is invalid";
                let invalid_tooltip =
                    "This theme contains invalid CSS. Re-import a corrected file or delete it";
                let edit_blocked = ambiguous_id || (!projected && (!valid || selection_blocked));
                let edit_label = if projected && editor_open {
                    "Show editor"
                } else {
                    "Edit here"
                };
                let edit_tooltip = if ambiguous_id {
                    identity_tooltip
                } else if !valid && !projected {
                    invalid_tooltip
                } else if projected {
                    if editor_open {
                        "Show the open Theme CSS tab"
                    } else {
                        "Edit this theme in API Tester"
                    }
                } else {
                    theme_selection_tooltip(
                        detached_theme,
                        editor_pending,
                        external_draft_pending,
                        "Use and edit this theme in API Tester",
                    )
                };
                let external_tooltip = if ambiguous_id {
                    identity_tooltip
                } else if !valid && !projected {
                    invalid_tooltip
                } else if projected {
                    "Open this theme in your preferred CSS app"
                } else {
                    theme_selection_tooltip(
                        detached_theme,
                        editor_pending,
                        external_draft_pending,
                        "Use this theme and open it in your preferred CSS app",
                    )
                };
                let delete_tooltip = if ambiguous_id {
                    identity_tooltip
                } else {
                    theme_selection_tooltip(
                        detached_theme,
                        editor_pending,
                        external_draft_pending,
                        "Delete this saved theme after confirmation",
                    )
                };
                let status = if ambiguous_id {
                    theme_problem_badge("Unavailable", cx)
                } else if !valid {
                    theme_problem_badge(
                        if projected {
                            "Selected · invalid"
                        } else {
                            "Invalid CSS"
                        },
                        cx,
                    )
                } else if active {
                    active_theme_badge(cx)
                } else {
                    available_theme_badge(cx)
                };
                let use_this = this.clone();
                let edit_this = this.clone();
                let external_this = this.clone();
                let discard_external_this = this.clone();
                let delete_this = this.clone();
                let use_theme_id = theme_id.clone();
                let edit_theme_id = theme_id.clone();
                let external_theme_id = theme_id.clone();
                let delete_theme_id = theme_id;
                let actions = h_flex()
                    .w_full()
                    .justify_end()
                    .gap_1()
                    .when(!active && valid && !ambiguous_id, |actions| {
                        actions.child(
                            Button::new(use_element_id)
                                .label("Use theme")
                                .small()
                                .outline()
                                .disabled(!writable || selection_blocked)
                                .tooltip(theme_selection_tooltip(
                                    detached_theme,
                                    editor_pending,
                                    external_draft_pending,
                                    "Switch to this saved theme",
                                ))
                                .on_click(move |_, _, cx| {
                                    if let Some(this) = use_this.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.switch_css_theme(Some(use_theme_id.clone()), cx);
                                        });
                                    }
                                }),
                        )
                    })
                    .child(
                        Button::new(edit_element_id)
                            .label(edit_label)
                            .small()
                            .outline()
                            .disabled(!writable || edit_blocked)
                            .tooltip(edit_tooltip)
                            .on_click(move |_, window, cx| {
                                if let Some(this) = edit_this.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.edit_saved_theme_here(
                                            edit_theme_id.clone(),
                                            window,
                                            cx,
                                        );
                                    });
                                }
                            }),
                    )
                    .child(
                        Button::new(external_element_id)
                            .label("Edit in preferred editor")
                            .small()
                            .ghost()
                            .disabled(!writable || edit_blocked)
                            .tooltip(external_tooltip)
                            .on_click(move |_, _, cx| {
                                if let Some(this) = external_this.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.edit_saved_theme_externally(
                                            external_theme_id.clone(),
                                            cx,
                                        );
                                    });
                                }
                            }),
                    )
                    .when(projected && external_draft_pending, |actions| {
                        actions.child(
                            Button::new(discard_external_element_id)
                                .label("Ignore file changes")
                                .small()
                                .ghost()
                                .disabled(!writable)
                                .tooltip("Keep the file, but stop watching it for changes")
                                .on_click(move |_, _, cx| {
                                    if let Some(this) = discard_external_this.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.discard_external_theme_draft(cx);
                                        });
                                    }
                                }),
                        )
                    })
                    .child(
                        Button::new(delete_element_id)
                            .label("Delete…")
                            .small()
                            .ghost()
                            .text_color(cx.theme().danger)
                            .disabled(!writable || selection_blocked || ambiguous_id)
                            .tooltip(delete_tooltip)
                            .on_click(move |_, window, cx| {
                                if let Some(this) = delete_this.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.delete_saved_theme(
                                            delete_theme_id.clone(),
                                            window,
                                            cx,
                                        );
                                    });
                                }
                            }),
                    )
                    .into_any_element();
                rows.push(theme_table_row(
                    row_key,
                    theme.name.clone(),
                    source,
                    status,
                    actions,
                    cx,
                ));
            }

            div()
                .id("theme-table-scroll")
                .w_full()
                .overflow_x_scroll()
                .child(
                    v_flex()
                        .w_full()
                        .min_w(px(THEME_TABLE_MIN_WIDTH))
                        .rounded_lg()
                        .border_1()
                        .border_color(cx.api_outline_variant())
                        .overflow_hidden()
                        .bg(cx.api_surface())
                        .child(theme_table_header(cx))
                        .children(rows),
                )
                .into_any_element()
        })
    }
}

const THEME_TABLE_MIN_WIDTH: f32 = 1_420.;
const THEME_TABLE_NAME_WIDTH: f32 = 280.;
const THEME_TABLE_STATUS_WIDTH: f32 = 160.;
const THEME_TABLE_ACTIONS_WIDTH: f32 = 720.;

fn theme_table_header(cx: &App) -> AnyElement {
    h_flex()
        .h(px(36.))
        .w_full()
        .flex_shrink_0()
        .bg(cx.api_surface_low())
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(
            div()
                .w(px(THEME_TABLE_NAME_WIDTH))
                .h_full()
                .flex_shrink_0()
                .px_3()
                .flex()
                .items_center()
                .border_r_1()
                .border_color(cx.api_outline_variant())
                .child("THEME"),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(260.))
                .h_full()
                .px_3()
                .flex()
                .items_center()
                .border_r_1()
                .border_color(cx.api_outline_variant())
                .child("SOURCE"),
        )
        .child(
            div()
                .w(px(THEME_TABLE_STATUS_WIDTH))
                .h_full()
                .flex_shrink_0()
                .px_3()
                .flex()
                .items_center()
                .border_r_1()
                .border_color(cx.api_outline_variant())
                .child("STATUS"),
        )
        .child(
            div()
                .w(px(THEME_TABLE_ACTIONS_WIDTH))
                .h_full()
                .flex_shrink_0()
                .px_3()
                .flex()
                .items_center()
                .justify_end()
                .child("ACTIONS"),
        )
        .into_any_element()
}

fn theme_table_row(
    row_key: impl Into<SharedString>,
    name: String,
    source: String,
    status: AnyElement,
    actions: AnyElement,
    cx: &App,
) -> AnyElement {
    let row_key = row_key.into();
    let row_id: SharedString = format!("theme-table-row-{row_key}").into();
    let name_id: SharedString = format!("theme-table-name-{row_key}").into();
    let source_id: SharedString = format!("theme-table-source-{row_key}").into();

    h_flex()
        .id(row_id)
        .w_full()
        .min_w(px(THEME_TABLE_MIN_WIDTH))
        .h(px(54.))
        .flex_shrink_0()
        .border_t_1()
        .border_color(cx.api_outline_variant())
        .bg(cx.api_surface())
        .hover(|style| style.bg(cx.api_surface_low()))
        .child(
            div()
                .id(name_id)
                .w(px(THEME_TABLE_NAME_WIDTH))
                .h_full()
                .flex_shrink_0()
                .min_w_0()
                .px_3()
                .flex()
                .items_center()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_sm()
                .font_medium()
                .border_r_1()
                .border_color(cx.api_outline_variant())
                .tooltip({
                    let name = name.clone();
                    move |window, cx| Tooltip::new(name.clone()).build(window, cx)
                })
                .child(name),
        )
        .child(
            div()
                .id(source_id)
                .flex_1()
                .min_w(px(260.))
                .h_full()
                .px_3()
                .flex()
                .items_center()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .border_r_1()
                .border_color(cx.api_outline_variant())
                .tooltip({
                    let source = source.clone();
                    move |window, cx| Tooltip::new(source.clone()).build(window, cx)
                })
                .child(source),
        )
        .child(
            div()
                .w(px(THEME_TABLE_STATUS_WIDTH))
                .h_full()
                .flex_shrink_0()
                .px_3()
                .flex()
                .items_center()
                .border_r_1()
                .border_color(cx.api_outline_variant())
                .child(status),
        )
        .child(
            div()
                .w(px(THEME_TABLE_ACTIONS_WIDTH))
                .h_full()
                .flex_shrink_0()
                .px_3()
                .flex()
                .items_center()
                .child(actions),
        )
        .into_any_element()
}

fn active_theme_badge(cx: &App) -> AnyElement {
    h_flex()
        .gap_1()
        .px_2()
        .py_1()
        .rounded_full()
        .bg(cx.theme().primary.opacity(0.14))
        .text_xs()
        .font_semibold()
        .text_color(cx.api_primary_bright())
        .child(Icon::new(IconName::Check).xsmall())
        .child("Active")
        .into_any_element()
}

fn available_theme_badge(cx: &App) -> AnyElement {
    div()
        .px_2()
        .py_1()
        .rounded_full()
        .bg(cx.api_surface_high())
        .text_xs()
        .font_medium()
        .text_color(cx.theme().muted_foreground)
        .child("Available")
        .into_any_element()
}

fn theme_problem_badge(label: &'static str, cx: &App) -> AnyElement {
    div()
        .px_2()
        .py_1()
        .rounded_full()
        .bg(cx.theme().danger.opacity(0.12))
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().danger)
        .child(label)
        .into_any_element()
}

fn theme_selection_tooltip(
    detached_theme: bool,
    editor_pending: bool,
    external_draft_pending: bool,
    available: &'static str,
) -> &'static str {
    if detached_theme {
        "Save this theme before switching"
    } else if external_draft_pending {
        "Reload or ignore the file changes before switching themes"
    } else if editor_pending {
        "Save or revert your changes before switching themes"
    } else {
        available
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
