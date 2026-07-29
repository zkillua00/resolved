use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

use crate::core::ThemeSettings;

use super::*;

impl ApiTester {
    pub(super) fn choose_css_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.has_detached_theme_snapshot() {
            self.settings_notice = Some(
                "Save the current unsaved CSS snapshot as a theme before importing another.".into(),
            );
            cx.notify();
            return;
        }
        if self.has_unapplied_theme_draft() {
            self.settings_notice =
                Some("Save or revert the in-app CSS draft before importing another theme.".into());
            cx.notify();
            return;
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose CSS theme".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let result = read_css_theme(&path);
            let _ = this.update(cx, |this, cx| match result {
                Ok(source) => {
                    this.dismiss_theme_editor();
                    this.install_css_theme(Some(path.clone()), source, None, cx);
                }
                Err(error) => {
                    this.settings_notice = Some(error);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn reload_css_theme(&mut self, cx: &mut Context<Self>) {
        if self.has_detached_theme_snapshot() {
            self.settings_notice = Some(
                "Save the unreconciled CSS snapshot as a new theme before reloading from a file."
                    .into(),
            );
            cx.notify();
            return;
        }
        if self.has_unapplied_editor_draft() {
            self.settings_notice = Some(
                "Open the recovered CSS draft, then save or revert it before reloading.".into(),
            );
            cx.notify();
            return;
        }
        let Some(path) = self
            .settings
            .theme
            .draft_path
            .clone()
            .or_else(|| self.settings.theme.source_path.clone())
        else {
            self.settings_notice = Some("This theme has no source file to reload.".to_owned());
            cx.notify();
            return;
        };
        match read_css_theme(&path) {
            Ok(source) => {
                let active_theme_id = self.settings.theme.active_theme_id.clone();
                self.dismiss_theme_editor();
                self.install_css_theme(Some(path), source, active_theme_id, cx);
            }
            Err(error) => {
                self.settings_notice = Some(error);
                cx.notify();
            }
        }
    }

    pub(super) fn switch_css_theme(&mut self, theme_id: Option<String>, cx: &mut Context<Self>) {
        if self.has_detached_theme_snapshot() {
            self.settings_notice = Some(
                "Save the current unsaved CSS snapshot as a theme before switching away from it."
                    .into(),
            );
            cx.notify();
            return;
        }
        if self.has_unapplied_theme_draft() {
            self.settings_notice =
                Some("Save or revert the CSS draft before switching themes.".into());
            cx.notify();
            return;
        }

        let selected = match theme_id.as_deref() {
            Some(id) => {
                let Some(theme) = self.settings.theme.saved_theme(id).cloned() else {
                    self.settings_notice =
                        Some("That saved theme no longer exists in the library.".into());
                    cx.notify();
                    return;
                };
                let parsed = match crate::theme::parse_css(&theme.css_source) {
                    Ok(parsed) => parsed,
                    Err(error) => {
                        self.settings_notice =
                            Some(format!("Saved theme “{}” is invalid: {error}", theme.name));
                        cx.notify();
                        return;
                    }
                };
                Some((theme, parsed))
            }
            None => None,
        };

        let mut candidate = self.settings.clone();
        candidate.theme.draft_source = None;
        candidate.theme.draft_path = None;
        candidate.theme.draft_disk_source = None;
        match selected.as_ref() {
            Some((theme, _)) => {
                candidate.theme.active_theme_id = Some(theme.id.clone());
                candidate.theme.source_path = theme.source_path.clone();
                candidate.theme.css_source = Some(theme.css_source.clone());
            }
            None => {
                candidate.theme.active_theme_id = None;
                candidate.theme.source_path = None;
                candidate.theme.css_source = None;
            }
        }

        match self.commit_settings(candidate, false, cx) {
            Ok(()) => {
                self.dismiss_theme_editor();
                match selected {
                    Some((theme, parsed)) => {
                        crate::theme::apply(parsed, cx);
                        self.settings_notice = Some(format!("Using saved theme “{}”.", theme.name));
                    }
                    None => {
                        crate::theme::configure(cx);
                        self.settings_notice =
                            Some("Using built-in API Tester Material Dark.".to_owned());
                    }
                }
                self.refresh_variable_intelligence(cx);
            }
            Err(error) => self.settings_notice = Some(error),
        }
        cx.notify();
    }

    pub(super) fn open_theme_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.settings_writable {
            self.settings_notice =
                Some("The CSS editor is unavailable while settings are read-only.".into());
            cx.notify();
            return;
        }
        if let Some(editor) = self.theme_editor.clone() {
            self.open_workspace_tool_tab(WorkspaceToolTab::ThemeCss, window, cx);
            editor.read(cx).focus_handle(cx).focus(window);
            return;
        }

        let baseline = self.current_theme_source().to_owned();
        let draft_path = self.settings.theme.draft_path.clone();
        let path = draft_path
            .clone()
            .or_else(|| self.settings.theme.source_path.clone());
        let disk_source = path.as_deref().and_then(|path| read_css_theme(path).ok());
        let source = self
            .settings
            .theme
            .draft_source
            .clone()
            .or_else(|| {
                draft_path
                    .as_deref()
                    .and_then(|path| read_css_theme(path).ok())
            })
            .unwrap_or_else(|| baseline.clone());
        let recovered = source != baseline;
        let intelligence = Rc::new(crate::theme::ThemeCssIntelligence);
        let editor = cx.new(|cx| {
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(CodeLanguage::Css)
                    .initial_value(source.clone())
                    .placeholder("Define one :root theme")
                    .rows(28)
                    .soft_wrap(false)
                    .completion_provider(intelligence.clone())
                    .hover_provider(intelligence)
                    .diagnostic_provider(crate::theme::theme_css_diagnostics),
                window,
                cx,
            )
        });
        let subscription = cx.subscribe(&editor, |this, _, event: &InputEvent, cx| {
            if !matches!(event, InputEvent::Change) {
                return;
            }
            this.theme_editor_changed(cx);
        });

        let path_is_active = path.as_ref() == self.settings.theme.source_path.as_ref();
        self.theme_editor_path = path;
        self.theme_editor_baseline = baseline;
        self.theme_editor_dirty = recovered;
        self.theme_editor_disk_source = trusted_theme_disk_source(
            disk_source,
            self.settings.theme.draft_disk_source.as_deref(),
            &source,
            &self.theme_editor_baseline,
            path_is_active,
        );
        self.theme_editor_subscription = Some(subscription);
        self.theme_editor = Some(editor.clone());
        self.open_workspace_tool_tab(WorkspaceToolTab::ThemeCss, window, cx);
        self.settings_notice = Some(if recovered {
            "Recovered the unapplied CSS draft. Save or revert it when ready.".into()
        } else {
            "CSS intelligence is active. Saving validates before changing the theme.".into()
        });
        editor.read(cx).focus_handle(cx).focus(window);
        cx.notify();
    }

    pub(super) fn apply_theme_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.theme_editor.as_ref() else {
            return;
        };
        if self.has_detached_theme_snapshot() {
            self.settings_notice = Some(
                "The active CSS snapshot is not safely linked to that library entry. Use Save as new theme instead."
                    .into(),
            );
            cx.notify();
            return;
        }
        let source = editor.read(cx).value(cx).to_string();
        let parsed = match crate::theme::parse_css(&source) {
            Ok(theme) => theme,
            Err(error) => {
                self.settings_notice = Some(format!("Theme was not applied: {error}"));
                cx.notify();
                return;
            }
        };
        let Some(active_id) = self.settings.theme.active_theme_id.clone() else {
            self.settings_notice =
                Some("The built-in theme is read-only. Use Save as new theme instead.".into());
            cx.notify();
            return;
        };
        if self.settings.theme.saved_theme(&active_id).is_none() {
            self.settings_notice = Some(
                "The selected theme is missing from the library. Choose another theme before saving."
                    .into(),
            );
            cx.notify();
            return;
        }
        let theme_name = parsed.name.to_string();
        let mut candidate = self.settings.clone();
        candidate.theme.source_path = None;
        candidate.theme.css_source = Some(source.clone());
        candidate.theme.draft_source = None;
        candidate.theme.draft_path = None;
        candidate.theme.draft_disk_source = None;
        let saved = candidate
            .theme
            .saved_themes
            .iter_mut()
            .find(|saved| saved.id == active_id)
            .expect("active saved theme was checked before writing its source");
        saved.css_source = source.clone();
        saved.source_path = None;
        match self.commit_settings(candidate, false, cx) {
            Ok(()) => {
                crate::theme::apply(parsed, cx);
                self.refresh_variable_intelligence(cx);
                self.theme_editor_path = None;
                self.theme_editor_baseline = source.clone();
                self.theme_editor_dirty = false;
                self.theme_editor_disk_source = None;
                self.theme_editor_persist_task = None;
                self.settings_notice = Some(format!("Applied and saved “{theme_name}”."));
            }
            Err(error) => self.settings_notice = Some(error),
        }
        cx.notify();
    }

    pub(super) fn open_save_theme_as_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.theme_editor.as_ref() else {
            self.settings_notice = Some("Open the CSS editor before saving a theme.".into());
            cx.notify();
            return;
        };
        let source = editor.read(cx).value(cx).to_string();
        let parsed = match crate::theme::parse_css(&source) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.settings_notice = Some(format!("Theme cannot be saved yet: {error}"));
                cx.notify();
                return;
            }
        };
        let default_name =
            next_available_theme_name(&self.settings.theme.saved_themes, parsed.name.as_ref());
        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Theme name")
                .default_value(default_name)
        });
        let this = cx.entity().downgrade();
        let input_for_dialog = name_input.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let save_this = this.clone();
            let input_for_save = input_for_dialog.clone();
            dialog
                .title("Save as new theme")
                .w(px(460.))
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("Save theme"))
                .on_ok(move |_, _, cx| {
                    let name = input_for_save.read(cx).value().trim().to_owned();
                    if name.is_empty() || name.chars().count() > 80 {
                        return false;
                    }
                    let Some(this) = save_this.upgrade() else {
                        return true;
                    };
                    this.update(cx, |this, cx| {
                        this.save_theme_editor_as_new(name, cx);
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
                                .child(
                                    "Save this valid CSS snapshot in the theme library. Names may be up to 80 characters.",
                                ),
                        )
                        .child(Input::new(&input_for_dialog)),
                )
        });
        name_input.read(cx).focus_handle(cx).focus(window);
    }

    fn save_theme_editor_as_new(&mut self, requested_name: String, cx: &mut Context<Self>) {
        if !self.settings_writable {
            self.settings_notice =
                Some("The theme library is read-only because settings could not be loaded.".into());
            cx.notify();
            return;
        }
        let Some(editor) = self.theme_editor.as_ref() else {
            return;
        };
        let source = editor.read(cx).value(cx).to_string();
        let parsed = match crate::theme::parse_css(&source) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.settings_notice = Some(format!("Theme was not saved: {error}"));
                cx.notify();
                return;
            }
        };
        let name = next_available_theme_name(&self.settings.theme.saved_themes, &requested_name);
        let saved = SavedTheme::new(name.clone(), source.clone(), None);
        let saved_id = saved.id.clone();
        let mut candidate = self.settings.clone();
        candidate.theme.saved_themes.push(saved);
        candidate.theme.active_theme_id = Some(saved_id);
        candidate.theme.source_path = None;
        candidate.theme.css_source = Some(source.clone());
        candidate.theme.draft_source = None;
        candidate.theme.draft_path = None;
        candidate.theme.draft_disk_source = None;

        match self.commit_settings(candidate, false, cx) {
            Ok(()) => {
                crate::theme::apply(parsed, cx);
                self.refresh_variable_intelligence(cx);
                self.theme_editor_path = None;
                self.theme_editor_baseline = source.clone();
                self.theme_editor_dirty = false;
                self.theme_editor_disk_source = None;
                self.theme_editor_persist_task = None;
                self.settings_notice = Some(format!("Saved and selected “{name}”."));
            }
            Err(error) => self.settings_notice = Some(error),
        }
        cx.notify();
    }

    pub(super) fn delete_active_saved_theme(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.has_detached_theme_snapshot() {
            self.settings_notice = Some(
                "Save the unreconciled CSS snapshot as a new theme before deleting the selected library entry."
                    .into(),
            );
            cx.notify();
            return;
        }
        if self.has_unapplied_theme_draft() {
            self.settings_notice =
                Some("Save or revert the CSS draft before deleting a theme.".into());
            cx.notify();
            return;
        }
        let Some(active_id) = self.settings.theme.active_theme_id.clone() else {
            return;
        };
        let Some(theme) = self.settings.theme.saved_theme(&active_id) else {
            return;
        };
        let name = theme.name.clone();
        let removal_message = saved_theme_removal_message(theme);
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let delete_this = this.clone();
            let theme_id = active_id.clone();
            let theme_name = name.clone();
            dialog
                .title("Remove saved theme?")
                .w(px(460.))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Remove theme")
                        .ok_variant(ButtonVariant::Danger),
                )
                .on_ok(move |_, _, cx| {
                    if let Some(this) = delete_this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.remove_saved_theme(&theme_id, &theme_name, cx);
                        });
                    }
                    true
                })
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(removal_message.clone()),
                )
        });
    }

    fn remove_saved_theme(&mut self, theme_id: &str, theme_name: &str, cx: &mut Context<Self>) {
        let mut candidate = self.settings.clone();
        let Some(index) = candidate
            .theme
            .saved_themes
            .iter()
            .position(|theme| theme.id == theme_id)
        else {
            self.settings_notice = Some("That saved theme no longer exists.".into());
            cx.notify();
            return;
        };
        candidate.theme.saved_themes.remove(index);
        let was_active = candidate.theme.active_theme_id.as_deref() == Some(theme_id);
        if was_active {
            candidate.theme.active_theme_id = None;
            candidate.theme.source_path = None;
            candidate.theme.css_source = None;
            candidate.theme.draft_source = None;
            candidate.theme.draft_path = None;
            candidate.theme.draft_disk_source = None;
        }
        match self.commit_settings(candidate, false, cx) {
            Ok(()) => {
                if was_active {
                    self.dismiss_theme_editor();
                    crate::theme::configure(cx);
                    self.refresh_variable_intelligence(cx);
                }
                self.settings_notice =
                    Some(format!("Removed “{theme_name}” from the theme library."));
            }
            Err(error) => self.settings_notice = Some(error),
        }
        cx.notify();
    }

    pub(super) fn revert_theme_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.theme_editor.as_ref() else {
            return;
        };
        let baseline = self.theme_editor_baseline.clone();
        editor.update(cx, |editor, cx| {
            editor.set_value(baseline.clone(), window, cx);
        });
        self.theme_editor_path = self.settings.theme.source_path.clone();
        self.theme_editor_dirty = false;
        self.theme_editor_disk_source = self
            .theme_editor_path
            .as_deref()
            .and_then(|path| read_css_theme(path).ok())
            .filter(|disk| disk == &baseline);
        self.theme_editor_persist_task = None;
        let mut candidate = self.settings.clone();
        candidate.theme.draft_source = None;
        candidate.theme.draft_path = None;
        candidate.theme.draft_disk_source = None;
        self.settings_notice = Some(match self.commit_settings(candidate, false, cx) {
            Ok(()) => "Reverted the CSS draft to the active theme.".into(),
            Err(error) => {
                format!("The editor was reverted, but draft recovery could not be cleared: {error}")
            }
        });
        cx.notify();
    }

    pub(super) fn restore_default_theme_template(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.theme_editor.as_ref() else {
            return;
        };
        editor.update(cx, |editor, cx| {
            editor.set_value(crate::theme::bundled_css(), window, cx);
        });
        self.theme_editor_changed(cx);
        self.settings_notice = Some(
            "Loaded the documented default into the editor. Save changes or save it as a new theme to use it."
                .into(),
        );
        cx.notify();
    }

    pub(super) fn reload_theme_editor_from_disk(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.theme_editor_dirty {
            let this = cx.entity().downgrade();
            window.open_dialog(cx, move |dialog, _, cx| {
                let reload_this = this.clone();
                dialog
                    .title("Replace the in-app CSS draft?")
                    .w(px(460.))
                    .confirm()
                    .button_props(
                        DialogButtonProps::default()
                            .ok_text("Reload from disk")
                            .ok_variant(ButtonVariant::Danger),
                    )
                    .on_ok(move |_, window, cx| {
                        if let Some(this) = reload_this.upgrade() {
                            this.update(cx, |this, cx| {
                                this.reload_theme_editor_from_disk_now(window, cx);
                            });
                        }
                        true
                    })
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                "The file on disk will replace the current editor buffer and its recoverable draft. This cannot be undone.",
                            ),
                    )
            });
            return;
        }
        self.reload_theme_editor_from_disk_now(window, cx);
    }

    fn reload_theme_editor_from_disk_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.theme_editor_path.clone() else {
            self.settings_notice = Some("This draft has no file to reload.".into());
            cx.notify();
            return;
        };
        let source = match read_css_theme(&path) {
            Ok(source) => source,
            Err(error) => {
                self.settings_notice = Some(error);
                cx.notify();
                return;
            }
        };
        let Some(editor) = self.theme_editor.as_ref() else {
            return;
        };
        editor.update(cx, |editor, cx| {
            editor.set_value(source.clone(), window, cx);
        });
        self.theme_editor_disk_source = Some(source);
        self.theme_editor_changed(cx);
        if !self.persist_theme_editor_draft(cx) {
            cx.notify();
            return;
        }
        self.settings_notice =
            Some("Reloaded the file into the CSS editor. Review it, then save when ready.".into());
        cx.notify();
    }

    pub(super) fn open_css_in_preferred_editor(&mut self, cx: &mut Context<Self>) {
        if !self.settings_writable {
            self.settings_notice = Some(
                "The preferred-editor source cannot be prepared while settings are read-only."
                    .into(),
            );
            cx.notify();
            return;
        }
        let editor_source = self
            .theme_editor
            .as_ref()
            .map(|editor| editor.read(cx).value(cx).to_string());
        let source = editor_source
            .or_else(|| self.settings.theme.draft_source.clone())
            .unwrap_or_else(|| self.current_theme_source().to_owned());
        let persisted_path = self.settings.theme.source_path.clone();
        let draft_pending = source != self.current_theme_source();
        let path = self
            .theme_editor_path
            .clone()
            .or_else(|| self.settings.theme.draft_path.clone())
            .or_else(|| persisted_path.clone())
            .filter(|path| !(draft_pending && persisted_path.as_ref() == Some(path)));
        let mut path = match path {
            Some(path) => path,
            None => next_available_managed_theme_path(draft_pending),
        };

        if draft_pending {
            match inspect_external_theme_source(
                &path,
                self.theme_editor_disk_source.as_deref(),
                &source,
            ) {
                Ok(ThemeSourceState::Ready) => {}
                Ok(ThemeSourceState::Missing | ThemeSourceState::NeedsFreshPath) => {
                    path = next_available_managed_theme_path(true);
                    if let Err(error) = materialize_theme_source(&path, source.as_bytes()) {
                        self.settings_notice = Some(error);
                        cx.notify();
                        return;
                    }
                }
                Err(error) => {
                    self.settings_notice = Some(error);
                    cx.notify();
                    return;
                }
            }
        } else if !path.exists()
            && let Err(error) = materialize_theme_source(&path, source.as_bytes())
        {
            path = next_available_managed_theme_path(false);
            if let Err(second_error) = materialize_theme_source(&path, source.as_bytes()) {
                self.settings_notice = Some(format!("{error} {second_error}"));
                cx.notify();
                return;
            }
        }

        self.theme_editor_path = Some(path.clone());
        self.theme_editor_disk_source = Some(source.clone());
        let persisted = if self.theme_editor.is_some() {
            self.persist_theme_editor_draft(cx)
        } else {
            let mut candidate = self.settings.clone();
            if candidate.theme.source_path.as_ref() != Some(&path) {
                candidate.theme.draft_path = Some(path.clone());
                candidate.theme.draft_disk_source = Some(source.clone());
            }
            if draft_pending {
                candidate.theme.draft_source = Some(
                    self.settings
                        .theme
                        .draft_source
                        .clone()
                        .unwrap_or_else(|| self.current_theme_source().to_owned()),
                );
            }
            self.commit_settings(candidate, false, cx)
                .map(|()| true)
                .unwrap_or_else(|error| {
                    self.settings_notice = Some(error);
                    false
                })
        };
        if !persisted {
            cx.notify();
            return;
        }
        cx.open_with_system(&path);
        self.settings_notice = Some(format!(
            "Opened {} with the macOS preferred CSS editor. Use Reload from disk before saving if both editors are open.",
            path.display()
        ));
        cx.notify();
    }

    pub(super) fn close_theme_editor(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.persist_theme_editor_draft(cx) {
            cx.notify();
            return false;
        }
        let draft_saved = self.settings.theme.draft_source.is_some();
        self.dismiss_theme_editor();
        self.settings_notice = Some(if draft_saved {
            "CSS editor closed. The unapplied draft is saved and will reopen where you left it."
                .into()
        } else {
            "CSS editor closed.".into()
        });
        cx.notify();
        true
    }

    pub(super) fn has_unapplied_theme_draft(&self) -> bool {
        self.has_unapplied_editor_draft() || self.settings.theme.draft_path.is_some()
    }

    pub(super) fn has_unapplied_editor_draft(&self) -> bool {
        self.theme_editor_dirty || self.settings.theme.draft_source.is_some()
    }

    pub(super) fn discard_external_theme_draft(&mut self, cx: &mut Context<Self>) {
        if self.has_unapplied_editor_draft() {
            self.settings_notice =
                Some("Save or revert the in-app CSS draft before discarding its file link.".into());
            cx.notify();
            return;
        }
        let Some(path) = self.settings.theme.draft_path.clone() else {
            return;
        };
        let mut candidate = self.settings.clone();
        candidate.theme.draft_path = None;
        candidate.theme.draft_disk_source = None;
        match self.commit_settings(candidate, false, cx) {
            Ok(()) => {
                self.theme_editor_path = self.settings.theme.source_path.clone();
                self.theme_editor_disk_source = self
                    .theme_editor_path
                    .as_deref()
                    .and_then(|source_path| read_css_theme(source_path).ok());
                self.settings_notice = Some(format!(
                    "Stopped tracking {}. The file was left on disk.",
                    path.display()
                ));
            }
            Err(error) => self.settings_notice = Some(error),
        }
        cx.notify();
    }

    pub(super) fn has_detached_theme_snapshot(&self) -> bool {
        theme_snapshot_is_detached(&self.settings.theme)
    }

    pub(crate) fn flush_local_state(&mut self, cx: &mut Context<Self>) -> bool {
        let tabs_saved = self.flush_request_tabs(cx);
        let theme_saved = self.persist_theme_editor_draft(cx);
        if !theme_saved {
            self.settings_notice = Some("The CSS draft could not be saved.".into());
            cx.notify();
        }
        tabs_saved && theme_saved
    }

    fn current_theme_source(&self) -> &str {
        match self.settings.theme.css_source.as_deref() {
            Some(source) => source,
            None => crate::theme::bundled_css(),
        }
    }

    fn dismiss_theme_editor(&mut self) {
        self.theme_editor = None;
        self.theme_editor_path = None;
        self.theme_editor_baseline.clear();
        self.theme_editor_dirty = false;
        self.theme_editor_disk_source = None;
        self.theme_editor_validation_task = None;
        self.theme_editor_persist_task = None;
        self.theme_editor_subscription = None;
        self.workspace_tabs
            .close_tool(WorkspaceToolTab::ThemeCss, false);
    }

    fn theme_editor_changed(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.theme_editor.as_ref() else {
            return;
        };
        let dirty = editor.read(cx).value(cx).as_ref() != self.theme_editor_baseline.as_str();
        if dirty != self.theme_editor_dirty {
            self.theme_editor_dirty = dirty;
            cx.notify();
        }
        self.theme_editor_validation_task = Some(cx.spawn(async move |this, cx| {
            Timer::after(THEME_EDITOR_VALIDATION_DEBOUNCE).await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |_, cx| cx.notify()).ok();
            }
        }));
        self.theme_editor_persist_task = Some(cx.spawn(async move |this, cx| {
            Timer::after(THEME_EDITOR_PERSIST_DEBOUNCE).await;
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                this.theme_editor_persist_task = None;
                if !this.persist_theme_editor_draft(cx) {
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn persist_theme_editor_draft(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(editor) = self.theme_editor.as_ref() else {
            return true;
        };
        let source = editor.read(cx).value(cx).to_string();
        let dirty = source != self.theme_editor_baseline;
        self.theme_editor_dirty = dirty;
        let mut candidate = self.settings.clone();
        candidate.theme.draft_source = dirty.then_some(source);
        let draft_path = self
            .theme_editor_path
            .clone()
            .filter(|path| candidate.theme.source_path.as_ref() != Some(path));
        candidate.theme.draft_path = draft_path.clone();
        candidate.theme.draft_disk_source = (dirty || draft_path.is_some())
            .then(|| self.theme_editor_disk_source.clone())
            .flatten();
        if candidate == self.settings {
            return true;
        }
        match self.commit_settings(candidate, false, cx) {
            Ok(()) => true,
            Err(error) => {
                self.settings_notice =
                    Some(format!("CSS draft recovery could not be saved: {error}"));
                false
            }
        }
    }

    fn install_css_theme(
        &mut self,
        source_path: Option<PathBuf>,
        source: String,
        preferred_theme_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let parsed = match crate::theme::parse_css(&source) {
            Ok(theme) => theme,
            Err(error) => {
                self.settings_notice = Some(format!("Theme was not applied: {error}"));
                cx.notify();
                return;
            }
        };
        let parsed_name = parsed.name.to_string();
        let mut candidate = self.settings.clone();
        let existing_index = preferred_theme_id
            .as_deref()
            .and_then(|theme_id| {
                candidate
                    .theme
                    .saved_themes
                    .iter()
                    .position(|theme| theme.id == theme_id)
            })
            .or_else(|| {
                source_path.as_ref().and_then(|source_path| {
                    candidate
                        .theme
                        .saved_themes
                        .iter()
                        .position(|theme| theme.source_path.as_ref() == Some(source_path))
                })
            });
        let (theme_id, theme_name) = if let Some(index) = existing_index {
            let saved = &mut candidate.theme.saved_themes[index];
            saved.css_source = source.clone();
            saved.source_path = source_path.clone();
            (saved.id.clone(), saved.name.clone())
        } else {
            let theme_name = next_available_theme_name(&candidate.theme.saved_themes, &parsed_name);
            let saved = SavedTheme::new(theme_name.clone(), source.clone(), source_path.clone());
            let theme_id = saved.id.clone();
            candidate.theme.saved_themes.push(saved);
            (theme_id, theme_name)
        };
        candidate.theme.active_theme_id = Some(theme_id);
        candidate.theme.source_path = source_path;
        candidate.theme.css_source = Some(source);
        candidate.theme.draft_source = None;
        candidate.theme.draft_path = None;
        candidate.theme.draft_disk_source = None;
        match self.commit_settings(candidate, false, cx) {
            Ok(()) => {
                crate::theme::apply(parsed, cx);
                self.refresh_variable_intelligence(cx);
                self.settings_notice = Some(format!("Applied “{theme_name}”."));
            }
            Err(error) => self.settings_notice = Some(error),
        }
        cx.notify();
    }
}

fn theme_snapshot_is_detached(theme: &ThemeSettings) -> bool {
    match theme.active_theme_id.as_deref() {
        None => theme.css_source.is_some(),
        Some(active_id) => theme.saved_theme(active_id).is_none_or(|saved| {
            theme.css_source.as_deref() != Some(saved.css_source.as_str())
                || theme.source_path != saved.source_path
        }),
    }
}

fn saved_theme_removal_message(theme: &SavedTheme) -> String {
    match theme.source_path.as_ref().filter(|path| path.is_file()) {
        Some(path) => format!(
            "“{}” will be removed from the theme library. Its source file at {} will be left on disk.",
            theme.name,
            path.display()
        ),
        None => format!(
            "“{}” is stored only in API Tester’s SQLite theme library. Removing it deletes the only saved copy known to API Tester and cannot be undone.",
            theme.name
        ),
    }
}

fn next_available_theme_name(themes: &[SavedTheme], requested: &str) -> String {
    const MAX_NAME_CHARS: usize = 80;

    let requested = requested.trim();
    let requested = if requested.is_empty() {
        "Untitled theme"
    } else {
        requested
    };
    let requested = requested.chars().take(MAX_NAME_CHARS).collect::<String>();
    let has_name = |candidate: &str| {
        themes
            .iter()
            .any(|theme| theme.name.eq_ignore_ascii_case(candidate))
    };
    if !has_name(&requested) {
        return requested;
    }
    for suffix in 2usize.. {
        let suffix = format!(" ({suffix})");
        let keep = MAX_NAME_CHARS.saturating_sub(suffix.chars().count());
        let base = requested.chars().take(keep).collect::<String>();
        let candidate = format!("{}{suffix}", base.trim_end());
        if !has_name(&candidate) {
            return candidate;
        }
    }
    unreachable!("theme name suffix search is unbounded")
}

fn read_css_theme(path: &Path) -> Result<String, String> {
    const MAX_THEME_BYTES: u64 = 256 * 1024;

    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("css"))
    {
        return Err("Theme files must use the .css extension.".to_owned());
    }
    let metadata = fs::metadata(path)
        .map_err(|error| format!("Theme file could not be inspected: {error}"))?;
    if metadata.len() > MAX_THEME_BYTES {
        return Err(format!(
            "Theme file is too large ({} bytes; maximum is {MAX_THEME_BYTES}).",
            metadata.len()
        ));
    }
    fs::read_to_string(path).map_err(|error| {
        format!(
            "Theme file could not be read as UTF-8 at {}: {error}",
            path.display()
        )
    })
}

fn managed_theme_path() -> PathBuf {
    DatabaseStore::default_path()
        .parent()
        .map_or_else(
            || PathBuf::from("themes"),
            |directory| directory.join("themes"),
        )
        .join("api-tester-theme.css")
}

fn next_available_managed_theme_path(draft: bool) -> PathBuf {
    let base = managed_theme_path();
    let stem = if draft {
        "api-tester-theme-draft"
    } else {
        "api-tester-theme"
    };
    next_available_theme_path(&base, stem)
}

fn next_available_theme_path(base: &Path, stem: &str) -> PathBuf {
    let first = base.with_file_name(format!("{stem}.css"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|index| base.with_file_name(format!("{stem}-{index}.css")))
        .find(|candidate| !candidate.exists())
        .expect("managed theme filename search is unbounded")
}

fn materialize_theme_source(path: &Path, source: &[u8]) -> Result<(), String> {
    ensure_theme_parent(path)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("api-tester-theme.css");
    let mut attempt = 0u32;
    let (temporary_path, mut temporary_file) = loop {
        let candidate = path.with_file_name(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            attempt
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => break (candidate, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && attempt < 16 => {
                attempt += 1;
            }
            Err(error) => {
                return Err(format!(
                    "Theme draft could not create a temporary file beside {}: {error}",
                    path.display()
                ));
            }
        }
    };
    let result: std::io::Result<()> = (|| {
        temporary_file.write_all(source)?;
        temporary_file.sync_all()?;
        drop(temporary_file);
        fs::hard_link(&temporary_path, path)?;
        if let Err(error) = fs::remove_file(&temporary_path) {
            tracing::warn!(
                path = %temporary_path.display(),
                %error,
                "published theme snapshot but could not remove its temporary hard link"
            );
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary_path);
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            return Err(format!(
                "Theme draft already exists at {}; it was left untouched.",
                path.display()
            ));
        }
        return Err(format!(
            "Theme draft could not be published safely to {}: {error}",
            path.display()
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThemeSourceState {
    Missing,
    Ready,
    NeedsFreshPath,
}

fn inspect_external_theme_source(
    path: &Path,
    expected_source: Option<&str>,
    desired_source: &str,
) -> Result<ThemeSourceState, String> {
    let current = match fs::read_to_string(path) {
        Ok(current) => current,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ThemeSourceState::Missing);
        }
        Err(error) => {
            return Err(format!(
                "Theme source could not be checked for external changes at {}: {error}",
                path.display()
            ));
        }
    };
    if current == desired_source {
        return Ok(ThemeSourceState::Ready);
    }
    if expected_source.is_some_and(|expected| expected == current) {
        return Ok(ThemeSourceState::NeedsFreshPath);
    }
    Err(format!(
        "{} changed outside API Tester. Reload from disk before opening the in-app draft externally; the external edits were left untouched.",
        path.display()
    ))
}

fn trusted_theme_disk_source(
    actual_source: Option<String>,
    persisted_expected_source: Option<&str>,
    editor_source: &str,
    active_baseline: &str,
    path_is_active_source: bool,
) -> Option<String> {
    actual_source.filter(|actual| {
        actual == editor_source
            || persisted_expected_source.is_some_and(|expected| expected == actual)
            || (path_is_active_source && actual == active_baseline)
    })
}

fn ensure_theme_parent(path: &Path) -> Result<(), String> {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "Theme directory could not be created at {}: {error}",
            parent.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_theme_materialization_never_overwrites_an_existing_draft() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("theme.css");
        fs::write(&path, b"user draft").unwrap();

        assert!(materialize_theme_source(&path, b"default template").is_err());

        assert_eq!(fs::read(&path).unwrap(), b"user draft");
    }

    #[test]
    fn external_change_detection_never_overwrites_unknown_content() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("theme.css");
        fs::write(&path, "external").unwrap();

        let error = inspect_external_theme_source(&path, Some("previous"), "in-app").unwrap_err();

        assert!(error.contains("changed outside API Tester"));
        assert_eq!(fs::read_to_string(path).unwrap(), "external");
    }

    #[test]
    fn a_known_unchanged_external_source_requests_a_fresh_path() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("theme.css");
        fs::write(&path, "previous").unwrap();

        assert_eq!(
            inspect_external_theme_source(&path, Some("previous"), "new in-app draft").unwrap(),
            ThemeSourceState::NeedsFreshPath
        );
        assert_eq!(fs::read_to_string(path).unwrap(), "previous");
    }

    #[test]
    fn managed_theme_allocation_skips_existing_sources() {
        let directory = tempfile::tempdir().unwrap();
        let base = directory.path().join("api-tester-theme.css");
        fs::write(&base, "first").unwrap();
        fs::write(directory.path().join("api-tester-theme-2.css"), "second").unwrap();

        assert_eq!(
            next_available_theme_path(&base, "api-tester-theme"),
            directory.path().join("api-tester-theme-3.css")
        );
        assert_eq!(fs::read_to_string(base).unwrap(), "first");
    }

    #[test]
    fn recovered_draft_trusts_only_the_disk_state_it_was_based_on() {
        assert_eq!(
            trusted_theme_disk_source(Some("active".into()), None, "dirty draft", "active", true,),
            Some("active".into())
        );
        assert_eq!(
            trusted_theme_disk_source(
                Some("managed draft".into()),
                Some("managed draft"),
                "newer in-app draft",
                "active",
                false,
            ),
            Some("managed draft".into())
        );
        assert_eq!(
            trusted_theme_disk_source(
                Some("external edit".into()),
                Some("managed draft"),
                "newer in-app draft",
                "active",
                false,
            ),
            None
        );
    }

    #[test]
    fn theme_materialization_publishes_complete_source_without_temporary_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested").join("theme.css");

        materialize_theme_source(&path, b"complete source").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"complete source");
        assert!(fs::read_dir(path.parent().unwrap()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
    }

    #[test]
    fn saved_theme_names_are_disambiguated_case_insensitively() {
        let themes = vec![
            SavedTheme::new("Ocean", "first", None),
            SavedTheme::new("Ocean (2)", "second", None),
        ];

        assert_eq!(next_available_theme_name(&themes, "ocean"), "ocean (3)");
        assert_eq!(next_available_theme_name(&themes, "Forest"), "Forest");
    }

    #[test]
    fn saved_theme_names_stay_within_the_ui_limit() {
        let long_name = "a".repeat(100);
        let themes = vec![SavedTheme::new("a".repeat(80), "first", None)];

        let candidate = next_available_theme_name(&themes, &long_name);

        assert_eq!(candidate.chars().count(), 80);
        assert!(candidate.ends_with(" (2)"));
    }

    #[test]
    fn detached_theme_detection_covers_missing_and_mismatched_active_projections() {
        let saved = SavedTheme::new("Saved", "saved source", Some("/tmp/saved.css".into()));
        let matching = ThemeSettings {
            saved_themes: vec![saved.clone()],
            active_theme_id: Some(saved.id.clone()),
            source_path: saved.source_path.clone(),
            css_source: Some(saved.css_source.clone()),
            ..Default::default()
        };
        assert!(!theme_snapshot_is_detached(&ThemeSettings::default()));
        assert!(!theme_snapshot_is_detached(&matching));

        let mut legacy_snapshot = ThemeSettings {
            css_source: Some("legacy source".into()),
            ..Default::default()
        };
        assert!(theme_snapshot_is_detached(&legacy_snapshot));

        legacy_snapshot.active_theme_id = Some("missing".into());
        assert!(theme_snapshot_is_detached(&legacy_snapshot));

        let mut mismatched_source = matching.clone();
        mismatched_source.css_source = Some("other source".into());
        assert!(theme_snapshot_is_detached(&mismatched_source));

        let mut mismatched_path = matching;
        mismatched_path.source_path = None;
        assert!(theme_snapshot_is_detached(&mismatched_path));
    }

    #[test]
    fn delete_warning_promises_a_file_only_when_it_currently_exists() {
        let directory = tempfile::tempdir().unwrap();
        let existing_path = directory.path().join("theme.css");
        fs::write(&existing_path, "source").unwrap();

        let sqlite_only = SavedTheme::new("SQLite only", "source", None);
        assert!(saved_theme_removal_message(&sqlite_only).contains("only saved copy"));

        let missing_file = SavedTheme::new(
            "Missing file",
            "source",
            Some(directory.path().join("missing.css")),
        );
        assert!(saved_theme_removal_message(&missing_file).contains("only saved copy"));

        let backed = SavedTheme::new("Backed", "source", Some(existing_path.clone()));
        let message = saved_theme_removal_message(&backed);
        assert!(message.contains("will be left on disk"));
        assert!(message.contains(&existing_path.display().to_string()));
    }
}
