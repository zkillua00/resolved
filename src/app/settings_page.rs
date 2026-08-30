use gpui_component::group_box::GroupBoxVariant;
use gpui_component::setting::{
    SettingField, SettingGroup, SettingItem, SettingPage, Settings as SettingsView,
};
use gpui_component::switch::Switch;

use super::*;

const SETTINGS_SIDEBAR_WIDTH: Pixels = px(220.);

impl ApiTester {
    /// Memoized parse of a theme's CSS source. Parsing happens once per
    /// distinct source instead of once per saved theme per frame.
    fn theme_parse_cached(
        &self,
        source: &str,
    ) -> Rc<Result<crate::theme::ApiTheme, crate::theme::ThemeError>> {
        let mut cache = self.theme_parse_cache.borrow_mut();
        if cache.len() >= 64 {
            cache.clear();
        }
        cache
            .entry(source.to_owned())
            .or_insert_with(|| Rc::new(crate::theme::parse_css(source)))
            .clone()
    }

    fn theme_parse_name(&self, source: &str) -> Option<String> {
        self.theme_parse_cached(source)
            .as_ref()
            .as_ref()
            .ok()
            .map(|theme| theme.name.to_string())
    }

    pub(super) fn render_settings_title_bar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .h(px(APP_TITLE_BAR_HEIGHT))
            .flex_shrink_0()
            .pl(windows_controls::leading_inset())
            .pr(windows_controls::trailing_inset())
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
                        .child("Settings"),
                ),
            )
            .child(windows_controls::caption_drag_region())
            .child(windows_controls::windows_window_controls(window, cx))
            .into_any_element()
    }

    pub(super) fn render_settings_workspace(&self, cx: &mut Context<Self>) -> AnyElement {
        let servers_page = self.upstream_settings_page(cx);
        let editor_page = SettingPage::new("Editor")
            .description(
                "Tune every code editor and the built-in JSON, JavaScript, and TypeScript formatters.",
            )
            .resettable(false)
            .group(
                SettingGroup::new()
                    .title("Editing")
                    .description("Changes apply immediately to every open editor.")
                    .items([
                        self.editor_tab_size_setting_item(cx),
                        self.editor_hard_tabs_setting_item(cx),
                        self.editor_soft_wrap_setting_item(cx),
                        self.editor_line_numbers_setting_item(cx),
                        self.editor_indent_guides_setting_item(cx),
                        self.editor_auto_close_pairs_setting_item(cx),
                    ]),
            )
            .group(
                SettingGroup::new()
                    .title("Formatting")
                    .description(
                        "Controls applied when formatting JSON, JavaScript, or TypeScript source.",
                    )
                    .items([
                        self.formatter_indent_size_setting_item(cx),
                        self.formatter_hard_tabs_setting_item(cx),
                        self.formatter_line_width_setting_item(cx),
                        self.formatter_quote_style_setting_item(cx),
                        self.formatter_semicolons_setting_item(cx),
                        self.formatter_trailing_commas_setting_item(cx),
                    ]),
            );
        let mut keyboard_page = SettingPage::new("Keyboard")
            .description("Record shortcuts directly. Defaults follow familiar macOS conventions.")
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
            .description("Enable diagnostics for inspecting Resolved while it is running.")
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

        let mut pages = Vec::with_capacity(5);
        pages.push(servers_page);
        pages.extend([editor_page, keyboard_page, appearance_page, developer_page]);

        v_flex()
            .relative()
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            .child(settings_sidebar_underlay(SETTINGS_SIDEBAR_WIDTH, cx))
            .when_some(self.settings_warning.clone(), |this, warning| {
                this.child(dismissible_settings_message(
                    warning,
                    cx.theme().danger,
                    SettingsMessageKind::Warning,
                    cx,
                ))
            })
            .when_some(self.settings_notice.clone(), |this, notice| {
                this.child(dismissible_settings_message(
                    notice,
                    cx.theme().info,
                    SettingsMessageKind::Notice,
                    cx,
                ))
            })
            .child(
                div().flex_1().min_h_0().child(
                    SettingsView::new("api-tester-settings")
                        .sidebar_width(SETTINGS_SIDEBAR_WIDTH)
                        .with_group_variant(GroupBoxVariant::Outline)
                        .pages(pages),
                ),
            )
            .into_any_element()
    }

    fn editor_tab_size_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let this = cx.entity().downgrade();
        SettingItem::new(
            "Tab size",
            SettingField::<SharedString>::render(move |_, _, cx| {
                let Some(entity) = this.upgrade() else {
                    return div().into_any_element();
                };
                let state = entity.read(cx);
                let selected = state.settings.editor.tab_size;
                let writable = state.settings_writable;
                let menu_this = this.clone();

                Button::new("editor-tab-size-picker")
                    .label(format!("{selected} spaces"))
                    .dropdown_caret(true)
                    .outline()
                    .w(px(220.))
                    .disabled(!writable)
                    .tooltip(settings_control_tooltip(
                        writable,
                        "Choose the visual width of a tab",
                    ))
                    .dropdown_menu(move |mut menu, _, _| {
                        menu = menu.min_w(px(220.));
                        for tab_size in [1_u8, 2, 4, 8] {
                            let item_this = menu_this.clone();
                            menu = menu.item(
                                PopupMenuItem::new(format!("{tab_size} spaces"))
                                    .checked(tab_size == selected)
                                    .on_click(move |_, window, cx| {
                                        if tab_size == selected {
                                            return;
                                        }
                                        if let Some(this) = item_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.set_editor_tab_size(tab_size, window, cx);
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
        .description("Sets indentation width and how wide existing tab characters appear.")
    }

    fn editor_hard_tabs_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let this = cx.entity().downgrade();
        SettingItem::new(
            "Use hard tabs",
            SettingField::<SharedString>::render(move |_, _, cx| {
                let Some(entity) = this.upgrade() else {
                    return div().into_any_element();
                };
                let state = entity.read(cx);
                let checked = state.settings.editor.hard_tabs;
                let writable = state.settings_writable;
                let change_this = this.clone();

                Switch::new("editor-hard-tabs")
                    .checked(checked)
                    .disabled(!writable)
                    .tooltip(settings_control_tooltip(
                        writable,
                        "Insert tabs instead of spaces when indenting",
                    ))
                    .on_click(move |checked, window, cx| {
                        if let Some(this) = change_this.upgrade() {
                            this.update(cx, |this, cx| {
                                this.set_editor_hard_tabs(*checked, window, cx);
                            });
                        }
                    })
                    .into_any_element()
            }),
        )
        .description("Insert tab characters for indentation. Tab size still controls their width.")
    }

    fn editor_soft_wrap_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let this = cx.entity().downgrade();
        SettingItem::new(
            "Soft wrap",
            SettingField::<SharedString>::render(move |_, _, cx| {
                let Some(entity) = this.upgrade() else {
                    return div().into_any_element();
                };
                let state = entity.read(cx);
                let checked = state.settings.editor.soft_wrap;
                let writable = state.settings_writable;
                let change_this = this.clone();

                Switch::new("editor-soft-wrap")
                    .checked(checked)
                    .disabled(!writable)
                    .tooltip(settings_control_tooltip(
                        writable,
                        "Wrap long lines within the editor viewport",
                    ))
                    .on_click(move |checked, window, cx| {
                        if let Some(this) = change_this.upgrade() {
                            this.update(cx, |this, cx| {
                                this.set_editor_soft_wrap(*checked, window, cx);
                            });
                        }
                    })
                    .into_any_element()
            }),
        )
        .description("Wrap long lines visually without changing their contents.")
    }

    fn editor_line_numbers_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let this = cx.entity().downgrade();
        SettingItem::new(
            "Line numbers",
            SettingField::<SharedString>::render(move |_, _, cx| {
                let Some(entity) = this.upgrade() else {
                    return div().into_any_element();
                };
                let state = entity.read(cx);
                let checked = state.settings.editor.line_numbers;
                let writable = state.settings_writable;
                let change_this = this.clone();

                Switch::new("editor-line-numbers")
                    .checked(checked)
                    .disabled(!writable)
                    .tooltip(settings_control_tooltip(
                        writable,
                        "Show line numbers beside code",
                    ))
                    .on_click(move |checked, window, cx| {
                        if let Some(this) = change_this.upgrade() {
                            this.update(cx, |this, cx| {
                                this.set_editor_line_numbers(*checked, window, cx);
                            });
                        }
                    })
                    .into_any_element()
            }),
        )
        .description("Show a line-number gutter in code editors.")
    }

    fn editor_indent_guides_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let this = cx.entity().downgrade();
        SettingItem::new(
            "Indent guides",
            SettingField::<SharedString>::render(move |_, _, cx| {
                let Some(entity) = this.upgrade() else {
                    return div().into_any_element();
                };
                let state = entity.read(cx);
                let checked = state.settings.editor.indent_guides;
                let writable = state.settings_writable;
                let change_this = this.clone();

                Switch::new("editor-indent-guides")
                    .checked(checked)
                    .disabled(!writable)
                    .tooltip(settings_control_tooltip(
                        writable,
                        "Show vertical indentation guides",
                    ))
                    .on_click(move |checked, window, cx| {
                        if let Some(this) = change_this.upgrade() {
                            this.update(cx, |this, cx| {
                                this.set_editor_indent_guides(*checked, window, cx);
                            });
                        }
                    })
                    .into_any_element()
            }),
        )
        .description("Show guides that make nested code structure easier to follow.")
    }

    fn editor_auto_close_pairs_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let this = cx.entity().downgrade();
        SettingItem::new(
            "Auto-close pairs",
            SettingField::<SharedString>::render(move |_, _, cx| {
                let Some(entity) = this.upgrade() else {
                    return div().into_any_element();
                };
                let state = entity.read(cx);
                let checked = state.settings.editor.auto_close_pairs;
                let writable = state.settings_writable;
                let change_this = this.clone();

                Switch::new("editor-auto-close-pairs")
                    .checked(checked)
                    .disabled(!writable)
                    .tooltip(settings_control_tooltip(
                        writable,
                        "Insert matching brackets and quotes",
                    ))
                    .on_click(move |checked, window, cx| {
                        if let Some(this) = change_this.upgrade() {
                            this.update(cx, |this, cx| {
                                this.set_editor_auto_close_pairs(*checked, window, cx);
                            });
                        }
                    })
                    .into_any_element()
            }),
        )
        .description("Automatically insert matching brackets, braces, parentheses, and quotes.")
    }

    fn formatter_indent_size_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let this = cx.entity().downgrade();
        SettingItem::new(
            "Indent size",
            SettingField::<SharedString>::render(move |_, _, cx| {
                let Some(entity) = this.upgrade() else {
                    return div().into_any_element();
                };
                let state = entity.read(cx);
                let selected = state.settings.formatter.indent_size;
                let writable = state.settings_writable;
                let menu_this = this.clone();

                Button::new("formatter-indent-size-picker")
                    .label(format!("{selected} spaces"))
                    .dropdown_caret(true)
                    .outline()
                    .w(px(220.))
                    .disabled(!writable)
                    .tooltip(settings_control_tooltip(
                        writable,
                        "Choose the indentation width produced by the formatter",
                    ))
                    .dropdown_menu(move |mut menu, _, _| {
                        menu = menu.min_w(px(220.));
                        for indent_size in [1_u8, 2, 4, 8] {
                            let item_this = menu_this.clone();
                            menu = menu.item(
                                PopupMenuItem::new(format!("{indent_size} spaces"))
                                    .checked(indent_size == selected)
                                    .on_click(move |_, _, cx| {
                                        if indent_size == selected {
                                            return;
                                        }
                                        if let Some(this) = item_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.set_formatter_indent_size(indent_size, cx);
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
        .description("Sets indentation width when formatting JSON, JavaScript, or TypeScript.")
    }

    fn formatter_hard_tabs_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let this = cx.entity().downgrade();
        SettingItem::new(
            "Use hard tabs",
            SettingField::<SharedString>::render(move |_, _, cx| {
                let Some(entity) = this.upgrade() else {
                    return div().into_any_element();
                };
                let state = entity.read(cx);
                let checked = state.settings.formatter.hard_tabs;
                let writable = state.settings_writable;
                let change_this = this.clone();

                Switch::new("formatter-hard-tabs")
                    .checked(checked)
                    .disabled(!writable)
                    .tooltip(settings_control_tooltip(
                        writable,
                        "Format indentation with tabs instead of spaces",
                    ))
                    .on_click(move |checked, _, cx| {
                        if let Some(this) = change_this.upgrade() {
                            this.update(cx, |this, cx| {
                                this.set_formatter_hard_tabs(*checked, cx);
                            });
                        }
                    })
                    .into_any_element()
            }),
        )
        .description("Emit tab characters for indentation in formatted output.")
    }

    fn formatter_line_width_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let this = cx.entity().downgrade();
        SettingItem::new(
            "Line width",
            SettingField::<SharedString>::render(move |_, _, cx| {
                let Some(entity) = this.upgrade() else {
                    return div().into_any_element();
                };
                let state = entity.read(cx);
                let selected = state.settings.formatter.line_width;
                let writable = state.settings_writable;
                let menu_this = this.clone();

                Button::new("formatter-line-width-picker")
                    .label(format!("{selected} columns"))
                    .dropdown_caret(true)
                    .outline()
                    .w(px(220.))
                    .disabled(!writable)
                    .tooltip(settings_control_tooltip(
                        writable,
                        "Choose the formatter's preferred maximum line width",
                    ))
                    .dropdown_menu(move |mut menu, _, _| {
                        menu = menu.min_w(px(220.));
                        for line_width in [40_u16, 60, 80, 100, 120, 160, 240] {
                            let item_this = menu_this.clone();
                            menu = menu.item(
                                PopupMenuItem::new(format!("{line_width} columns"))
                                    .checked(line_width == selected)
                                    .on_click(move |_, _, cx| {
                                        if line_width == selected {
                                            return;
                                        }
                                        if let Some(this) = item_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.set_formatter_line_width(line_width, cx);
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
        .description("Preferred maximum line width for JavaScript and TypeScript formatting.")
    }

    fn formatter_quote_style_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let this = cx.entity().downgrade();
        SettingItem::new(
            "JavaScript quotes",
            SettingField::<SharedString>::render(move |_, _, cx| {
                let Some(entity) = this.upgrade() else {
                    return div().into_any_element();
                };
                let state = entity.read(cx);
                let selected = state.settings.formatter.quote_style;
                let writable = state.settings_writable;
                let menu_this = this.clone();

                Button::new("formatter-quote-style-picker")
                    .label(selected.label())
                    .dropdown_caret(true)
                    .outline()
                    .w(px(220.))
                    .disabled(!writable)
                    .tooltip(settings_control_tooltip(
                        writable,
                        "Choose the preferred JavaScript string quote style",
                    ))
                    .dropdown_menu(move |mut menu, _, _| {
                        menu = menu.min_w(px(220.));
                        for quote_style in crate::core::FormatterQuoteStyle::ALL {
                            let item_this = menu_this.clone();
                            menu = menu.item(
                                PopupMenuItem::new(quote_style.label())
                                    .checked(quote_style == selected)
                                    .on_click(move |_, _, cx| {
                                        if quote_style == selected {
                                            return;
                                        }
                                        if let Some(this) = item_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.set_formatter_quote_style(
                                                    quote_style.key(),
                                                    cx,
                                                );
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
        .description(
            "Prefer double or single quotes when formatting JavaScript and TypeScript strings.",
        )
    }

    fn formatter_semicolons_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let this = cx.entity().downgrade();
        SettingItem::new(
            "Semicolons",
            SettingField::<SharedString>::render(move |_, _, cx| {
                let Some(entity) = this.upgrade() else {
                    return div().into_any_element();
                };
                let state = entity.read(cx);
                let selected = state.settings.formatter.semicolons;
                let writable = state.settings_writable;
                let menu_this = this.clone();

                Button::new("formatter-semicolon-picker")
                    .label(selected.label())
                    .dropdown_caret(true)
                    .outline()
                    .w(px(220.))
                    .disabled(!writable)
                    .tooltip(settings_control_tooltip(
                        writable,
                        "Choose whether formatted JavaScript prefers semicolons",
                    ))
                    .dropdown_menu(move |mut menu, _, _| {
                        menu = menu.min_w(px(220.));
                        for semicolons in crate::core::FormatterSemicolons::ALL {
                            let item_this = menu_this.clone();
                            menu = menu.item(
                                PopupMenuItem::new(semicolons.label())
                                    .checked(semicolons == selected)
                                    .on_click(move |_, _, cx| {
                                        if semicolons == selected {
                                            return;
                                        }
                                        if let Some(this) = item_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.set_formatter_semicolons(semicolons.key(), cx);
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
        .description(
            "Prefer explicit semicolons or rely on automatic insertion in JavaScript and TypeScript.",
        )
    }

    fn formatter_trailing_commas_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let this = cx.entity().downgrade();
        SettingItem::new(
            "Trailing commas",
            SettingField::<SharedString>::render(move |_, _, cx| {
                let Some(entity) = this.upgrade() else {
                    return div().into_any_element();
                };
                let state = entity.read(cx);
                let selected = state.settings.formatter.trailing_commas;
                let writable = state.settings_writable;
                let menu_this = this.clone();

                Button::new("formatter-trailing-commas-picker")
                    .label(selected.label())
                    .dropdown_caret(true)
                    .outline()
                    .w(px(220.))
                    .disabled(!writable)
                    .tooltip(settings_control_tooltip(
                        writable,
                        "Choose when formatted JavaScript uses trailing commas",
                    ))
                    .dropdown_menu(move |mut menu, _, _| {
                        menu = menu.min_w(px(220.));
                        for trailing_commas in crate::core::FormatterTrailingCommas::ALL {
                            let item_this = menu_this.clone();
                            menu = menu.item(
                                PopupMenuItem::new(trailing_commas.label())
                                    .checked(trailing_commas == selected)
                                    .on_click(move |_, _, cx| {
                                        if trailing_commas == selected {
                                            return;
                                        }
                                        if let Some(this) = item_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.set_formatter_trailing_commas(
                                                    trailing_commas.key(),
                                                    cx,
                                                );
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
        .description("Choose whether collections and parameter lists receive trailing commas.")
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
        .description("Choose the HUD corner. The location is restored when Resolved restarts.")
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
                let active_saved_theme = state
                    .settings
                    .theme
                    .active_theme_id
                    .as_deref()
                    .and_then(|theme_id| state.settings.theme.saved_theme(theme_id));
                let can_reload = active_saved_theme
                    .is_some_and(|theme| theme.source_path.is_some() || theme.draft_path.is_some())
                    || state.settings.theme.source_path.is_some()
                    || state.settings.theme.draft_path.is_some();
                let writable = state.settings_writable;
                let editor_pending = state.has_unscoped_theme_draft();
                let active_editor_pending = active_saved_theme
                    .is_some_and(|theme| theme.draft_source.is_some())
                    || state.theme_editors.values().any(|session| {
                        session.theme_id.as_deref()
                            == state.settings.theme.active_theme_id.as_deref()
                            && session.dirty
                    })
                    || editor_pending;
                let external_draft_pending = active_saved_theme
                    .is_some_and(|theme| theme.draft_path.is_some())
                    || state.settings.theme.draft_path.is_some();
                let selection_blocked = editor_pending || detached_theme;
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
                                    || active_editor_pending
                                    || detached_theme
                                    || active_id_ambiguous,
                            )
                            .tooltip(if !can_reload {
                                "The active theme has no file to reload"
                            } else if active_id_ambiguous {
                                "The active theme cannot be reloaded safely"
                            } else if active_editor_pending {
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
            "themes built in saved active invalid use edit preferred delete".to_owned();
        for theme in &self.settings.theme.saved_themes {
            search_text.push(' ');
            search_text.push_str(&theme.name);
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
            let detached_editor_open = state
                .theme_editors
                .values()
                .any(|session| session.theme_id.is_none());
            let editor_pending = state.has_unscoped_theme_draft();
            let external_draft_pending =
                state.settings.theme.draft_path.is_some() && !editor_pending;
            let selection_blocked = editor_pending || detached_theme;
            let mut rows = Vec::with_capacity(
                1 + usize::from(detached_theme) + state.settings.theme.saved_themes.len(),
            );

            let switch_built_in_this = this.clone();
            let discard_built_in_this = this.clone();
            let built_in_actions = h_flex()
                .w_full()
                .justify_end()
                .gap_1()
                .when(built_in_active, |actions| {
                    actions.child(active_theme_badge(cx))
                })
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
                "Resolved Material Dark".to_owned(),
                built_in_actions,
                cx,
            ));

            if detached_theme {
                let detached_name = state
                    .settings
                    .theme
                    .css_source
                    .as_deref()
                    .and_then(|source| state.theme_parse_name(source))
                    .unwrap_or_else(|| "Unsaved theme".to_owned());
                let edit_this = this.clone();
                let external_this = this.clone();
                let discard_this = this.clone();
                let detached_actions = h_flex()
                    .w_full()
                    .justify_end()
                    .gap_1()
                    .child(active_theme_badge(cx))
                    .child(
                        Button::new("edit-detached-css-theme-here")
                            .label(if detached_editor_open {
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
                                        this.edit_detached_theme_externally(cx);
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
                let valid = state.theme_parse_cached(&theme.css_source).is_ok();
                let active = projected && valid && !ambiguous_id;
                let editor_open = state.theme_editor_is_open_for(&theme_id);
                let theme_pending = theme.draft_source.is_some()
                    || theme.draft_path.is_some()
                    || state.theme_editors.values().any(|session| {
                        session.theme_id.as_deref() == Some(&theme_id) && session.dirty
                    });
                let theme_external_draft_pending =
                    theme.draft_path.is_some() && theme.draft_source.is_none();
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
                let edit_blocked = ambiguous_id || !valid;
                let edit_label = if editor_open {
                    "Show editor"
                } else {
                    "Edit here"
                };
                let edit_tooltip = if ambiguous_id {
                    identity_tooltip
                } else if !valid && !projected {
                    invalid_tooltip
                } else if editor_open {
                    "Show this theme’s editor tab"
                } else {
                    "Open this theme in a new editor tab"
                };
                let external_tooltip = if ambiguous_id {
                    identity_tooltip
                } else if !valid {
                    invalid_tooltip
                } else {
                    "Open this theme in your preferred CSS app"
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
                let use_this = this.clone();
                let edit_this = this.clone();
                let external_this = this.clone();
                let discard_external_this = this.clone();
                let delete_this = this.clone();
                let use_theme_id = theme_id.clone();
                let edit_theme_id = theme_id.clone();
                let external_theme_id = theme_id.clone();
                let discard_theme_id = theme_id.clone();
                let delete_theme_id = theme_id;
                let actions = h_flex()
                    .w_full()
                    .justify_end()
                    .gap_1()
                    .when(active, |actions| actions.child(active_theme_badge(cx)))
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
                    .when(theme_external_draft_pending, |actions| {
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
                                            this.discard_saved_theme_external_draft(
                                                discard_theme_id.clone(),
                                                cx,
                                            );
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
                            .disabled(!writable || theme_pending || ambiguous_id)
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
                rows.push(theme_table_row(row_key, theme.name.clone(), actions, cx));
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

const THEME_TABLE_MIN_WIDTH: f32 = 1_020.;
const THEME_TABLE_NAME_WIDTH: f32 = 320.;
const THEME_TABLE_ACTIONS_MIN_WIDTH: f32 = 700.;

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
                .min_w(px(THEME_TABLE_ACTIONS_MIN_WIDTH))
                .h_full()
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
    actions: AnyElement,
    cx: &App,
) -> AnyElement {
    let row_key = row_key.into();
    let row_id: SharedString = format!("theme-table-row-{row_key}").into();
    let name_id: SharedString = format!("theme-table-name-{row_key}").into();

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
                .flex_1()
                .min_w(px(THEME_TABLE_ACTIONS_MIN_WIDTH))
                .h_full()
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

fn settings_control_tooltip(writable: bool, available: &'static str) -> &'static str {
    if writable {
        available
    } else {
        "Settings are read-only because they could not be loaded safely"
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

#[derive(Clone, Copy)]
pub(super) enum SettingsMessageKind {
    Warning,
    Notice,
}

pub(super) fn dismissible_settings_message(
    message: String,
    color: Hsla,
    kind: SettingsMessageKind,
    cx: &mut Context<ApiTester>,
) -> AnyElement {
    let button_id = match kind {
        SettingsMessageKind::Warning => "dismiss-settings-warning",
        SettingsMessageKind::Notice => "dismiss-settings-notice",
    };

    h_flex()
        .mx_4()
        .mt_3()
        .px_3()
        .py_2()
        .gap_2()
        .rounded_md()
        .border_1()
        .border_color(color.opacity(0.45))
        .bg(color.opacity(0.1))
        .text_sm()
        .text_color(color)
        .child(div().flex_1().min_w_0().child(message))
        .child(
            Button::new(button_id)
                .icon(IconName::Close)
                .small()
                .ghost()
                .tooltip("Dismiss")
                .on_click(cx.listener(move |this, _, _, cx| {
                    match kind {
                        SettingsMessageKind::Warning => this.settings_warning = None,
                        SettingsMessageKind::Notice => this.settings_notice = None,
                    }
                    cx.notify();
                })),
        )
        .into_any_element()
}

pub(super) fn settings_sidebar_underlay(
    sidebar_width: Pixels,
    cx: &App,
) -> AnyElement {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left_0()
        .w(sidebar_width)
        .bg(cx.theme().sidebar)
        .border_r_1()
        .border_color(cx.theme().sidebar_border)
        .into_any_element()
}
