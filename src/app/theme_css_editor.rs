use gpui_component::setting::SettingItem;

use super::*;

impl ApiTester {
    pub(super) fn theme_editor_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        let this = cx.entity().downgrade();
        SettingItem::render(move |_, _, cx| {
            let Some(entity) = this.upgrade() else {
                return div().into_any_element();
            };
            let state = entity.read(cx);
            let Some(editor) = state.theme_editor.clone() else {
                return div().into_any_element();
            };
            let source = editor.read(cx).value(cx).to_string();
            let dirty = state.theme_editor_dirty;
            let writable = state.settings_writable;
            let path = state
                .theme_editor_path
                .as_ref()
                .or(state.settings.theme.source_path.as_ref())
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| {
                    "SQLite snapshot · Open in preferred editor creates a tracked CSS copy."
                        .to_owned()
                });
            let validation = crate::theme::parse_css(&source);
            let valid = validation.is_ok();
            let detached_theme = state.has_detached_theme_snapshot();
            let editing_saved_theme =
                state.settings.theme.active_theme_id.is_some() && !detached_theme;
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

            let apply_this = this.clone();
            let save_as_this = this.clone();
            let revert_this = this.clone();
            let default_this = this.clone();
            let external_this = this.clone();
            let reload_this = this.clone();
            let close_this = this.clone();

            v_flex()
                .w_full()
                .gap_3()
                .child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .gap_3()
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
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
                .child(div().w_full().h(px(520.)).child(editor))
                .child(
                    h_flex()
                        .w_full()
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
                                .tooltip(
                                    "Save this CSS as a separate theme and switch to it",
                                )
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
                            Button::new("close-css-editor")
                                .label("Close editor")
                                .ghost()
                                .tooltip("Release the editor; unapplied work remains recoverable")
                                .on_click(move |_, _, cx| {
                                    if let Some(this) = close_this.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.close_theme_editor(cx);
                                        });
                                    }
                                }),
                        )
                        .child(
                            Button::new("reload-css-editor-from-disk")
                                .label("Reload from disk")
                                .ghost()
                                .disabled(state.theme_editor_path.is_none())
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
                )
                .into_any_element()
        })
    }
}
