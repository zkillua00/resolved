use super::*;

impl ApiTester {
    pub(super) fn render_theme_css_title_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let title = self
            .workspace_tabs
            .active_theme_editor_id()
            .and_then(|editor_id| self.theme_editor_title(editor_id))
            .unwrap_or_else(|| "Theme CSS".to_owned());
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
                    .child(resolved_brand_lockup(cx))
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
                            .child(title),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_theme_css_workspace(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(session) = self.active_theme_editor() else {
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
        let editor = session.editor.clone();
        let theme_id = session.theme_id.clone();
        let editor_path = session.path.clone();

        let source = editor.read(cx).value(cx).to_string();
        let dirty = session.dirty;
        let writable = self.settings_writable;
        let saved_source_path = theme_id
            .as_deref()
            .and_then(|theme_id| self.settings.theme.saved_theme(theme_id))
            .and_then(|theme| theme.source_path.as_ref());
        let path = editor_path
            .as_ref()
            .or(saved_source_path)
            .or_else(|| {
                theme_id
                    .is_none()
                    .then_some(self.settings.theme.source_path.as_ref())
                    .flatten()
            })
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "Saved in Resolved".to_owned());
        let validation = crate::theme::parse_css(&source);
        let valid = validation.is_ok();
        let editing_saved_theme = theme_id
            .as_deref()
            .is_some_and(|theme_id| self.settings.theme.saved_theme(theme_id).is_some());
        let detached_theme = !editing_saved_theme;
        let (status, status_color) = match validation {
            Ok(theme) => (
                format!(
                    "{}{}",
                    theme.name,
                    if dirty { " · unapplied changes" } else { "" }
                ),
                if dirty {
                    cx.theme().warning
                } else {
                    cx.theme().success
                },
            ),
            Err(error) => (format!("Invalid CSS: {error}"), cx.theme().danger),
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
                    .child(
                        h_flex()
                            .id("theme-css-editor-toolbar")
                            .w_full()
                            .flex_shrink_0()
                            .overflow_x_scroll()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .border_b_1()
                            .border_color(cx.api_outline_variant())
                            .bg(cx.api_surface_low())
                            .child(
                                Button::new("apply-css-editor-theme")
                                    .label("Save changes")
                                    .small()
                                    .primary()
                                    .disabled(
                                        !writable || !editing_saved_theme || !dirty || !valid,
                                    )
                                    .tooltip(if editing_saved_theme {
                                        "Save your changes to this theme"
                                    } else if detached_theme {
                                        "Save this as a new theme before applying it"
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
                                    .small()
                                    .outline()
                                    .disabled(!writable || !valid)
                                    .tooltip("Save a copy as a new theme and use it")
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
                                    .small()
                                    .outline()
                                    .disabled(!dirty)
                                    .tooltip("Undo changes since the last save")
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
                                    .small()
                                    .ghost()
                                    .disabled(using_default_template)
                                    .tooltip("Replace the editor contents with the default theme")
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
                                    .small()
                                    .ghost()
                                    .disabled(!writable)
                                    .tooltip("Open this theme in your preferred CSS app")
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
                                    .label("Reload file")
                                    .small()
                                    .ghost()
                                    .disabled(editor_path.is_none())
                                    .tooltip(
                                        "Replace the editor contents with the latest version of the file",
                                    )
                                    .on_click(move |_, window, cx| {
                                        if let Some(this) = reload_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.reload_theme_editor_from_disk(window, cx);
                                            });
                                        }
                                    }),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .id("theme-css-editor-source-path")
                                    .max_w(px(280.))
                                    .flex_shrink_0()
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
                                    .max_w(px(320.))
                                    .flex_shrink_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
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
                            .bg(cx.api_surface_lowest())
                            .child(editor),
                    ),
            )
            .into_any_element()
    }
}
