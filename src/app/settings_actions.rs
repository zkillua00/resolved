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

    pub(super) fn set_metrics_position(
        &mut self,
        position: crate::core::MetricsPosition,
        cx: &mut Context<Self>,
    ) {
        if self.settings.metrics_position == position {
            return;
        }
        let mut candidate = self.settings.clone();
        candidate.metrics_position = position;
        match self.commit_settings(candidate, false, cx) {
            Ok(()) => {
                self.settings_notice = Some(format!("Metrics HUD moved to {}.", position.label()));
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
        self.debug_overlay.update(cx, |overlay, cx| {
            overlay.set_position(candidate.metrics_position, cx);
        });
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
    if let Some(catalog_warning) = theme_catalog_warning(settings) {
        warnings.push(catalog_warning);
    }
    (!warnings.is_empty()).then(|| warnings.join("\n"))
}

pub(super) fn theme_catalog_warning(settings: &AppSettings) -> Option<String> {
    let mut warnings = Vec::new();
    let mut theme_ids = BTreeSet::new();
    let mut source_paths = BTreeSet::new();
    for theme in &settings.theme.saved_themes {
        if theme.id.trim().is_empty()
            || theme.name.trim().is_empty()
            || !theme_ids.insert(theme.id.as_str())
        {
            warnings.push(
                "The saved theme library contains a missing name or a missing or duplicate identifier; affected themes cannot be selected safely."
                    .to_owned(),
            );
            break;
        }
        if let Some(path) = theme.source_path.as_ref()
            && !source_paths.insert(path)
        {
            warnings.push(format!(
                "More than one saved theme references {}; reload cannot safely choose which entry to update.",
                path.display()
            ));
            break;
        }
    }
    if let Some(theme) = settings
        .theme
        .saved_themes
        .iter()
        .find(|theme| crate::theme::parse_css(&theme.css_source).is_err())
    {
        warnings.push(format!(
            "Saved theme “{}” contains invalid CSS and cannot be selected until it is replaced.",
            theme.name
        ));
    }
    if let Some(active_id) = settings.theme.active_theme_id.as_deref() {
        match settings.theme.saved_theme(active_id) {
            Some(active) => {
                if settings.theme.css_source.as_deref() != Some(active.css_source.as_str())
                    || settings.theme.source_path != active.source_path
                {
                    warnings.push(format!(
                        "Selected theme “{}” does not match the active CSS snapshot; reselect it to restore a consistent theme.",
                        active.name
                    ));
                }
            }
            None => warnings.push(
                "The selected saved theme is missing from the theme library; choose another theme."
                    .to_owned(),
            ),
        }
    }
    (!warnings.is_empty()).then(|| warnings.join("\n"))
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
    fn semantic_warning_validates_saved_theme_identity_and_active_projection() {
        let mut settings = AppSettings::default();
        let saved = SavedTheme::new("Saved", crate::theme::bundled_css(), None);
        settings.theme.active_theme_id = Some(saved.id.clone());
        settings.theme.css_source = Some(crate::theme::bundled_css().to_owned());
        settings.theme.saved_themes.push(saved.clone());
        assert!(semantic_settings_warning(&settings).is_none());

        settings.theme.saved_themes.push(saved);
        let warning = semantic_settings_warning(&settings).unwrap();
        assert!(warning.contains("duplicate identifier"));

        settings.theme.saved_themes.pop();
        settings.theme.css_source = Some(":root {}".into());
        let warning = semantic_settings_warning(&settings).unwrap();
        assert!(warning.contains("does not match the active CSS snapshot"));
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
