use gpui_component::setting::{SettingField, SettingItem};

use super::*;

impl ApiTester {
    pub(crate) fn on_zoom_ui_in(
        &mut self,
        _: &shortcuts::ZoomUiIn,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.adjust_zoom(true, 1, cx);
    }

    pub(crate) fn on_zoom_ui_out(
        &mut self,
        _: &shortcuts::ZoomUiOut,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.adjust_zoom(true, -1, cx);
    }

    pub(crate) fn on_zoom_ui_reset(
        &mut self,
        _: &shortcuts::ZoomUiReset,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.adjust_zoom(true, 0, cx);
    }

    pub(crate) fn on_zoom_editor_in(
        &mut self,
        _: &shortcuts::ZoomEditorIn,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.adjust_zoom(false, 1, cx);
    }

    pub(crate) fn on_zoom_editor_out(
        &mut self,
        _: &shortcuts::ZoomEditorOut,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.adjust_zoom(false, -1, cx);
    }

    pub(crate) fn on_zoom_editor_reset(
        &mut self,
        _: &shortcuts::ZoomEditorReset,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.adjust_zoom(false, 0, cx);
    }

    /// Adjust one zoom surface by `ZOOM_STEP_PERCENT` steps and persist it.
    ///
    /// `steps == 0` resets that surface to the baseline percentage. The theme
    /// is re-applied from its stored source so the new scale takes effect
    /// immediately without ever rewriting the user's theme CSS.
    fn adjust_zoom(&mut self, ui: bool, steps: i16, cx: &mut Context<Self>) {
        if !self.settings_writable {
            self.settings_notice =
                Some("Zoom is read-only because settings could not be loaded safely.".to_owned());
            cx.notify();
            return;
        }
        let previous = self.settings.zoom.clone();
        let mut candidate = self.settings.clone();
        if steps == 0 {
            if ui {
                candidate.zoom.ui = crate::core::DEFAULT_ZOOM_PERCENT;
            } else {
                candidate.zoom.editor = crate::core::DEFAULT_ZOOM_PERCENT;
            }
        } else if ui {
            candidate.zoom.step_ui(steps);
        } else {
            candidate.zoom.step_editor(steps);
        }
        if candidate.zoom == previous {
            return;
        }
        match self.commit_settings(candidate, false, cx) {
            Ok(()) => self.reapply_theme_zoom(ui, cx),
            Err(error) => self.settings_notice = Some(error),
        }
        cx.notify();
    }

    /// Publish the persisted zoom and re-apply the active theme at the new
    /// scale so every open window repaints with it live.
    fn reapply_theme_zoom(&mut self, ui: bool, cx: &mut Context<Self>) {
        crate::theme::set_zoom(
            crate::theme::ThemeZoom {
                ui: self.settings.zoom.effective_ui(),
                editor: self.settings.zoom.effective_editor(),
            },
            cx,
        );
        match self.settings.theme.css_source.as_deref() {
            Some(source) => {
                if let Err(error) = crate::theme::parse_and_apply(source, cx) {
                    self.settings_notice = Some(format!(
                        "Zoom changed, but the active theme could not be re-applied at the new size: {error}"
                    ));
                    return;
                }
            }
            None => crate::theme::configure(cx),
        }
        let (label, percent) = if ui {
            ("Interface zoom", self.settings.zoom.ui)
        } else {
            ("Editor zoom", self.settings.zoom.editor)
        };
        self.settings_notice = Some(format!("{label} set to {percent}%."));
    }

    /// Settings row exposing the interface zoom controls.
    pub(super) fn ui_zoom_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        zoom_setting_item(cx, "Interface zoom", "ui-zoom", true)
            .description("Zoom the whole interface; the assigned shortcut is shown under Keyboard.")
    }

    /// Settings row exposing the code editor zoom controls.
    pub(super) fn editor_zoom_setting_item(&self, cx: &mut Context<Self>) -> SettingItem {
        zoom_setting_item(cx, "Editor zoom", "editor-zoom", false)
            .description("Adjust code editor text size independently; the assigned shortcut is shown under Keyboard.")
    }
}

/// Build the shared three-button zoom row used by the Editor and Appearance
/// settings pages. The row always re-reads the persisted percentage so it
/// reflects live changes and keyboard shortcuts immediately.
fn zoom_setting_item(
    cx: &mut Context<ApiTester>,
    title: &'static str,
    element_id: &'static str,
    editor: bool,
) -> SettingItem {
    let this = cx.entity().downgrade();
    let row_this = this.clone();
    let (in_id, out_id, reset_id) = match element_id {
        "ui-zoom" => ("ui-zoom-in", "ui-zoom-out", "ui-zoom-reset"),
        "editor-zoom" => ("editor-zoom-in", "editor-zoom-out", "editor-zoom-reset"),
        _ => unreachable!("zoom control ids are fixed"),
    };
    SettingItem::new(
        title,
        SettingField::<SharedString>::render(move |_, _, cx| {
            let Some(entity) = row_this.upgrade() else {
                return div().into_any_element();
            };
            let state = entity.read(cx);
            let writable = state.settings_writable;
            let current = if editor {
                state.settings.zoom.editor
            } else {
                state.settings.zoom.ui
            };
            let in_this = this.clone();
            let out_this = this.clone();
            let reset_this = this.clone();
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Button::new(in_id)
                        .label("Zoom in")
                        .small()
                        .outline()
                        .disabled(!writable)
                        .tooltip(zoom_control_tooltip(writable, "Make the interface larger"))
                        .on_click(move |_, window, cx| {
                            let _ = window;
                            if let Some(this) = in_this.upgrade() {
                                this.update(cx, |this, cx| this.adjust_zoom(editor, 1, cx));
                            }
                        }),
                )
                .child(
                    Button::new(out_id)
                        .label("Zoom out")
                        .small()
                        .outline()
                        .disabled(!writable)
                        .tooltip(zoom_control_tooltip(writable, "Make the interface smaller"))
                        .on_click(move |_, window, cx| {
                            let _ = window;
                            if let Some(this) = out_this.upgrade() {
                                this.update(cx, |this, cx| this.adjust_zoom(editor, -1, cx));
                            }
                        }),
                )
                .child(
                    div()
                        .min_w(px(52.))
                        .text_center()
                        .text_sm()
                        .child(format!("{current}%")),
                )
                .child(
                    Button::new(reset_id)
                        .label("Reset")
                        .small()
                        .ghost()
                        .disabled(!writable || current == crate::core::DEFAULT_ZOOM_PERCENT)
                        .tooltip(zoom_control_tooltip(
                            writable,
                            "Return to the theme's baseline size (100%)",
                        ))
                        .on_click(move |_, window, cx| {
                            let _ = window;
                            if let Some(this) = reset_this.upgrade() {
                                this.update(cx, |this, cx| this.adjust_zoom(editor, 0, cx));
                            }
                        }),
                )
                .into_any_element()
        }),
    )
}

fn zoom_control_tooltip(writable: bool, action: &'static str) -> &'static str {
    if writable {
        action
    } else {
        "Settings are read-only because they could not be loaded safely"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{px, size, TestAppContext};

    #[gpui::test]
    fn zoom_steps_persist_and_reapply_the_theme_at_the_new_scale(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("create temporary database directory");
        let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
        store.initialize().expect("initialize test database");

        let mut app = None;
        let store_for_app = store.clone();
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            let base_key_bindings = shortcuts::capture_base_key_bindings(cx);
            crate::theme::configure(cx);
            let view = cx.new(|cx| {
                ApiTester::new_with_database_store(base_key_bindings, store_for_app, window, cx)
            });
            crate::register_app_action_handlers(&view, cx);
            app = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        let app = app.expect("capture app entity");
        cx.update(|window, _| window.activate_window());
        cx.simulate_resize(size(px(1_200.), px(800.)));

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                // The bundled theme uses a 13 px editor font and a 16 px UI font.
                app.on_zoom_editor_in(&shortcuts::ZoomEditorIn, window, cx);
                assert_eq!(app.settings.zoom.ui, 100);
                assert_eq!(app.settings.zoom.editor, 110);
                let editor_font = crate::theme::GlobalApiTheme::get(cx)
                    .classes
                    .editor
                    .font_size;
                let ui_font = crate::theme::GlobalApiTheme::get(cx).classes.app.font_size;
                assert!(
                    (editor_font - px(13.0 * 1.1)).abs() < px(0.01),
                    "editor font scales with editor zoom"
                );
                assert!(
                    (ui_font - px(16.0)).abs() < px(0.01),
                    "interface font ignores editor zoom"
                );

                app.on_zoom_ui_out(&shortcuts::ZoomUiOut, window, cx);
                assert_eq!(app.settings.zoom.ui, 90);
                let ui_font = crate::theme::GlobalApiTheme::get(cx).classes.app.font_size;
                assert!(
                    (ui_font - px(16.0 * 0.9)).abs() < px(0.01),
                    "interface font scales with interface zoom"
                );

                app.on_zoom_editor_reset(&shortcuts::ZoomEditorReset, window, cx);
                assert_eq!(app.settings.zoom.editor, crate::core::DEFAULT_ZOOM_PERCENT);
                let editor_font = crate::theme::GlobalApiTheme::get(cx)
                    .classes
                    .editor
                    .font_size;
                assert!(
                    (editor_font - px(13.0)).abs() < px(0.01),
                    "editor font returns to baseline after reset"
                );
            });
        });

        let reloaded = store.load_app_settings().expect("reload persisted zoom");
        assert_eq!(reloaded.zoom.ui, 90);
        assert_eq!(reloaded.zoom.editor, crate::core::DEFAULT_ZOOM_PERCENT);
    }
}
