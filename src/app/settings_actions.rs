use std::{fs, path::Path};

use gpui::Keystroke;

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShortcutRecorderCommand {
    Cancel,
    Clear,
    Assign,
}

impl ApiTester {
    pub(super) fn begin_recording_shortcut(
        &mut self,
        shortcut_id: ShortcutId,
        cx: &mut Context<Self>,
    ) {
        self.recording_shortcut_id = Some(shortcut_id);
        self.settings_notice =
            Some("Press a shortcut. Escape cancels; Delete clears it.".to_owned());
        cx.notify();
    }

    pub(crate) fn cancel_shortcut_recording(&mut self, cx: &mut Context<Self>) {
        if self.recording_shortcut_id.take().is_some() {
            self.settings_notice = Some("Shortcut recording cancelled.".to_owned());
            cx.notify();
        }
    }

    pub(super) fn cancel_shortcut_recording_on_pointer(
        &mut self,
        _: &MouseDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_shortcut_recording(cx);
    }

    pub(super) fn capture_shortcut_keystroke(
        &mut self,
        keystroke: Keystroke,
        cx: &mut Context<Self>,
    ) {
        let Some(shortcut_id) = self.recording_shortcut_id.take() else {
            return;
        };
        match shortcut_recorder_command(&keystroke) {
            ShortcutRecorderCommand::Cancel => {
                self.settings_notice = Some("Shortcut recording cancelled.".to_owned());
                cx.notify();
            }
            ShortcutRecorderCommand::Clear => self.clear_shortcut(shortcut_id, cx),
            ShortcutRecorderCommand::Assign => {
                let raw = keystroke.unparse();
                let binding = match shortcuts::normalize_binding(&raw) {
                    Ok(binding) => binding,
                    Err(error) => {
                        self.settings_notice = Some(error.to_string());
                        cx.notify();
                        return;
                    }
                };
                let mut candidate = self.settings.clone();
                candidate.shortcuts.insert(
                    shortcut_id.key().to_owned(),
                    ShortcutOverride::Custom(binding.clone()),
                );
                match self.commit_settings(candidate, true, cx) {
                    Ok(()) => {
                        self.settings_notice =
                            Some(format!("{} now uses {}.", shortcut_id, binding));
                    }
                    Err(error) => self.settings_notice = Some(error),
                }
                cx.notify();
            }
        }
    }

    pub(super) fn clear_shortcut(&mut self, shortcut_id: ShortcutId, cx: &mut Context<Self>) {
        self.recording_shortcut_id = None;
        let mut candidate = self.settings.clone();
        candidate
            .shortcuts
            .insert(shortcut_id.key().to_owned(), ShortcutOverride::Disabled);
        match self.commit_settings(candidate, true, cx) {
            Ok(()) => {
                self.settings_notice = Some(format!("{} is now unassigned.", shortcut_id));
            }
            Err(error) => self.settings_notice = Some(error),
        }
        cx.notify();
    }

    pub(super) fn reset_shortcut(&mut self, shortcut_id: ShortcutId, cx: &mut Context<Self>) {
        self.recording_shortcut_id = None;
        let mut candidate = self.settings.clone();
        candidate.shortcuts.remove(shortcut_id.key());
        match self.commit_settings(candidate, true, cx) {
            Ok(()) => {
                self.settings_notice = Some(format!("{} restored to its default.", shortcut_id));
            }
            Err(error) => self.settings_notice = Some(error),
        }
        cx.notify();
    }

    pub(super) fn reset_all_shortcuts(&mut self, cx: &mut Context<Self>) {
        self.recording_shortcut_id = None;
        let mut candidate = self.settings.clone();
        for descriptor in shortcuts::shortcut_descriptors() {
            candidate.shortcuts.remove(descriptor.id.key());
        }
        match self.commit_settings(candidate, true, cx) {
            Ok(()) => self.settings_notice = Some("Default shortcuts restored.".to_owned()),
            Err(error) => self.settings_notice = Some(error),
        }
        cx.notify();
    }

    pub(super) fn persist_navigation_preference(&mut self, cx: &mut Context<Self>) {
        let previous = self.settings.navigation_compact;
        let mut candidate = self.settings.clone();
        candidate.navigation_compact = self.navigation_compact;
        if let Err(error) = self.commit_settings(candidate, false, cx) {
            self.navigation_compact = previous;
            self.settings_notice = Some(error);
        }
    }

    pub(super) fn choose_css_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                Ok(source) => this.install_css_theme(Some(path.clone()), source, cx),
                Err(error) => {
                    this.settings_notice = Some(error);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn reload_css_theme(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.settings.theme.source_path.clone() else {
            self.settings_notice = Some("This theme has no source file to reload.".to_owned());
            cx.notify();
            return;
        };
        match read_css_theme(&path) {
            Ok(source) => self.install_css_theme(Some(path), source, cx),
            Err(error) => {
                self.settings_notice = Some(error);
                cx.notify();
            }
        }
    }

    pub(super) fn reset_css_theme(&mut self, cx: &mut Context<Self>) {
        let mut candidate = self.settings.clone();
        candidate.theme = Default::default();
        match self.commit_settings(candidate, false, cx) {
            Ok(()) => {
                crate::theme::configure(cx);
                self.refresh_variable_intelligence(cx);
                self.settings_notice = Some("Built-in Material Dark theme restored.".to_owned());
            }
            Err(error) => self.settings_notice = Some(error),
        }
        cx.notify();
    }

    fn install_css_theme(
        &mut self,
        source_path: Option<std::path::PathBuf>,
        source: String,
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
        let theme_name = parsed.name.to_string();
        let mut candidate = self.settings.clone();
        candidate.theme.source_path = source_path;
        candidate.theme.css_source = Some(source);
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

    pub(super) fn commit_settings(
        &mut self,
        candidate: AppSettings,
        rebind_shortcuts: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.settings_writable {
            return Err(
                "Settings are read-only because they could not be loaded safely.".to_owned(),
            );
        }
        if rebind_shortcuts {
            shortcuts::effective_shortcuts(&candidate).map_err(|error| error.to_string())?;
        }
        self.database_store
            .save_app_settings(&candidate)
            .map_err(|error| format!("Settings could not be saved: {error}"))?;
        if rebind_shortcuts {
            shortcuts::apply_key_bindings(cx, &self.base_key_bindings, &candidate)
                .map_err(|error| format!("Shortcut keymap could not be applied: {error}"))?;
            crate::configure_menus(cx);
        }
        self.navigation_compact = candidate.navigation_compact;
        self.settings = candidate;
        self.settings_warning = semantic_settings_warning(&self.settings);
        Ok(())
    }
}

fn shortcut_recorder_command(keystroke: &Keystroke) -> ShortcutRecorderCommand {
    if keystroke.modifiers.number_of_modifiers() != 0 {
        return ShortcutRecorderCommand::Assign;
    }
    match keystroke.key.as_str() {
        "escape" => ShortcutRecorderCommand::Cancel,
        "backspace" | "delete" => ShortcutRecorderCommand::Clear,
        _ => ShortcutRecorderCommand::Assign,
    }
}

fn semantic_settings_warning(settings: &AppSettings) -> Option<String> {
    let mut warnings = Vec::new();
    if let Err(error) = shortcuts::effective_shortcuts(settings) {
        warnings.push(format!(
            "Stored shortcuts are invalid; defaults remain active until they are corrected: {error}"
        ));
    }
    if let Some(source) = settings.theme.css_source.as_deref()
        && let Err(error) = crate::theme::parse_css(source)
    {
        warnings.push(format!(
            "Stored CSS theme is invalid; the built-in theme remains active until it is corrected: {error}"
        ));
    }
    (!warnings.is_empty()).then(|| warnings.join("\n"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_warning_reports_each_invalid_settings_domain() {
        let mut settings = AppSettings::default();
        settings.shortcuts.insert(
            ShortcutId::NewRequestTab.key().to_owned(),
            ShortcutOverride::Custom("not-a-shortcut".to_owned()),
        );
        settings.theme.css_source = Some(":root { --api-theme-name: \"Broken\"; }".to_owned());

        let warning = semantic_settings_warning(&settings).unwrap();
        assert!(warning.contains("Stored shortcuts are invalid"));
        assert!(warning.contains("Stored CSS theme is invalid"));
    }

    #[test]
    fn semantic_warning_accepts_defaults_and_the_bundled_theme() {
        assert!(semantic_settings_warning(&AppSettings::default()).is_none());

        let mut settings = AppSettings::default();
        settings.theme.css_source = Some(crate::theme::bundled_css().to_owned());
        assert!(semantic_settings_warning(&settings).is_none());
    }

    #[test]
    fn recorder_controls_require_unmodified_escape_or_delete() {
        assert_eq!(
            shortcut_recorder_command(&Keystroke::parse("escape").unwrap()),
            ShortcutRecorderCommand::Cancel
        );
        assert_eq!(
            shortcut_recorder_command(&Keystroke::parse("delete").unwrap()),
            ShortcutRecorderCommand::Clear
        );
        for binding in ["cmd-escape", "cmd-delete", "alt-backspace"] {
            assert_eq!(
                shortcut_recorder_command(&Keystroke::parse(binding).unwrap()),
                ShortcutRecorderCommand::Assign,
                "{binding}"
            );
        }
    }
}
