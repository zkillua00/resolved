use super::*;

impl ApiTester {
    pub(super) fn render_theme_css_title_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .h(px(APP_TITLE_BAR_HEIGHT))
            .flex_shrink_0()
            .pl(px(92.))
            .pr_6()
            .border_b_1()
            .border_color(cx.theme().title_bar_border)
            .bg(cx.theme().title_bar)
            .child(
                h_flex()
                    .h_full()
                    .gap_6()
                    .items_center()
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
                            .gap_2()
                            .border_b_2()
                            .border_color(cx.theme().primary)
                            .px_1()
                            .text_sm()
                            .font_semibold()
                            .child(Icon::new(IconName::Palette).with_size(px(16.)))
                            .child("Theme CSS"),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_theme_css_workspace(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(editor) = self.theme_editor.clone() else {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .bg(cx.theme().background)
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("The Theme CSS editor is not open.")
                .into_any_element();
        };

        let source = editor.read(cx).value(cx).to_string();
        let dirty = self.theme_editor_dirty;
        let writable = self.settings_writable;
        let path = self
            .theme_editor_path
            .as_ref()
            .or(self.settings.theme.source_path.as_ref())
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| {
                "SQLite snapshot · Open in preferred editor creates a tracked CSS copy.".to_owned()
            });
        let validation = crate::theme::parse_css(&source);
        let valid = validation.is_ok();
        let detached_theme = self.has_detached_theme_snapshot();
        let editing_saved_theme = self.settings.theme.active_theme_id.is_some() && !detached_theme;
        let (status, status_color) = match validation {
            Ok(theme) => (
                format!(
                    "Valid theme: {}{}",
                    theme.name,
                    if dirty { " · unapplied changes" } else { "" }
                ),
                if dirty {
                    cx.theme().warning
                } else {
                    cx.theme().success
                },
            ),
            Err(error) => (format!("Not ready to save: {error}"), cx.theme().danger),
        };
        let using_default_template = source == crate::theme::bundled_css();
        let this = cx.entity().downgrade();

        let apply_this = this.clone();
        let save_as_this = this.clone();
        let revert_this = this.clone();
        let default_this = this.clone();
        let external_this = this.clone();
        let reload_this = this;

        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            .when_some(self.settings_warning.clone(), |this, warning| {
                this.child(super::settings_page::settings_message(
                    warning,
                    cx.theme().danger,
                ))
            })
            .when_some(self.settings_notice.clone(), |this, notice| {
                this.child(super::settings_page::settings_message(
                    notice,
                    cx.theme().info,
                ))
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_3()
                    .p_4()
                    .child(
                        h_flex()
                            .w_full()
                            .flex_shrink_0()
                            .justify_between()
                            .gap_3()
                            .child(
                                div()
                                    .id("theme-css-editor-source-path")
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .tooltip({
                                        let path = path.clone();
                                        move |window, cx| {
                                            Tooltip::new(path.clone()).build(window, cx)
                                        }
                                    })
                                    .child(path),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(status_color)
                                    .child(status),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex_1()
                            .min_h_0()
                            .rounded_lg()
                            .child(editor),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .flex_shrink_0()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                Button::new("apply-css-editor-theme")
                                    .label("Save changes")
                                    .primary()
                                    .disabled(
                                        !writable || !editing_saved_theme || !dirty || !valid,
                                    )
                                    .tooltip(if editing_saved_theme {
                                        "Validate and update the selected SQLite theme snapshot"
                                    } else if detached_theme {
                                        "This CSS is not safely linked to the selected library entry; save it as a new theme"
                                    } else {
                                        "The built-in theme is read-only; save this CSS as a new theme"
                                    })
                                    .on_click(move |_, _, cx| {
                                        if let Some(this) = apply_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.apply_theme_editor(cx);
                                            });
                                        }
                                    }),
                            )
                            .child(
                                Button::new("save-css-editor-theme-as")
                                    .label("Save as new theme…")
                                    .outline()
                                    .disabled(!writable || !valid)
                                    .tooltip("Save this CSS as a separate theme and switch to it")
                                    .on_click(move |_, window, cx| {
                                        if let Some(this) = save_as_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.open_save_theme_as_dialog(window, cx);
                                            });
                                        }
                                    }),
                            )
                            .child(
                                Button::new("revert-css-editor-theme")
                                    .label("Revert")
                                    .outline()
                                    .disabled(!dirty)
                                    .tooltip("Restore the last applied CSS snapshot")
                                    .on_click(move |_, window, cx| {
                                        if let Some(this) = revert_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.revert_theme_editor(window, cx);
                                            });
                                        }
                                    }),
                            )
                            .child(
                                Button::new("default-css-editor-template")
                                    .label("Default template")
                                    .ghost()
                                    .disabled(using_default_template)
                                    .tooltip("Load the fully documented built-in CSS template")
                                    .on_click(move |_, window, cx| {
                                        if let Some(this) = default_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.restore_default_theme_template(window, cx);
                                            });
                                        }
                                    }),
                            )
                            .child(
                                Button::new("open-css-editor-externally")
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
                                Button::new("reload-css-editor-from-disk")
                                    .label("Reload from disk")
                                    .ghost()
                                    .disabled(self.theme_editor_path.is_none())
                                    .tooltip(
                                        "Replace this editor buffer with the latest contents of its CSS file",
                                    )
                                    .on_click(move |_, window, cx| {
                                        if let Some(this) = reload_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.reload_theme_editor_from_disk(window, cx);
                                            });
                                        }
                                    }),
                            ),
                    ),
            )
            .into_any_element()
    }
}
