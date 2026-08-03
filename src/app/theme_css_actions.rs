use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

use crate::core::ThemeSettings;

use super::*;

const DETACHED_THEME_EDITOR_ID: &str = "detached-theme";

fn saved_theme_editor_id(theme_id: &str) -> String {
    format!("saved-theme-{theme_id}")
}

impl ApiTester {
    pub(super) fn active_theme_editor(&self) -> Option<&ThemeEditorSession> {
        self.workspace_tabs
            .active_theme_editor_id()
            .and_then(|editor_id| self.theme_editors.get(editor_id))
    }

    pub(super) fn theme_editor(&self, editor_id: &str) -> Option<&ThemeEditorSession> {
        self.theme_editors.get(editor_id)
    }

    pub(super) fn theme_editor_title(&self, editor_id: &str) -> Option<String> {
        self.theme_editors
            .get(editor_id)
            .map(|session| format!("Edit {}.css", session.theme_name))
    }

    pub(super) fn theme_editor_is_open_for(&self, theme_id: &str) -> bool {
        self.theme_editors
            .values()
            .any(|session| session.theme_id.as_deref() == Some(theme_id))
    }

    pub(super) fn choose_css_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.has_detached_theme_snapshot() {
            self.settings_notice = Some("Save this theme before importing another.".into());
            cx.notify();
            return;
        }
        if self.has_unscoped_theme_draft() {
            self.settings_notice =
                Some("Save or revert your changes before importing another theme.".into());
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
            self.settings_notice =
                Some("Save this as a new theme before reloading from a file.".into());
            cx.notify();
            return;
        }
        let active_theme_has_draft = self
            .settings
            .theme
            .active_theme_id
            .as_deref()
            .and_then(|theme_id| self.settings.theme.saved_theme(theme_id))
            .is_some_and(|theme| theme.draft_source.is_some() || theme.draft_path.is_some())
            || self.has_unscoped_theme_draft();
        if active_theme_has_draft {
            self.settings_notice = Some(
                "Open your restored changes, then save or revert them before reloading.".into(),
            );
            cx.notify();
            return;
        }
        let active_saved_theme = self
            .settings
            .theme
            .active_theme_id
            .as_deref()
            .and_then(|theme_id| self.settings.theme.saved_theme(theme_id));
        let Some(path) = active_saved_theme
            .and_then(|theme| theme.draft_path.clone())
            .or_else(|| active_saved_theme.and_then(|theme| theme.source_path.clone()))
            .or_else(|| self.settings.theme.draft_path.clone())
            .or_else(|| self.settings.theme.source_path.clone())
        else {
            self.settings_notice = Some("This theme has no file to reload.".to_owned());
            cx.notify();
            return;
        };
        match read_css_theme(&path) {
            Ok(source) => {
                let active_theme_id = self.settings.theme.active_theme_id.clone();
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
            self.settings_notice = Some("Save this theme before switching to another.".into());
            cx.notify();
            return;
        }
        if self.has_unscoped_theme_draft() {
            self.settings_notice =
                Some("Save or revert your changes before switching themes.".into());
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
                match selected {
                    Some((theme, parsed)) => {
                        crate::theme::apply(parsed, cx);
                        self.settings_notice = Some(format!("Using saved theme “{}”.", theme.name));
                    }
                    None => {
                        crate::theme::configure(cx);
                        self.settings_notice =
                            Some("Using built-in Resolved Material Dark.".to_owned());
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

        if !self.has_detached_theme_snapshot()
            && let Some(theme_id) = self.settings.theme.active_theme_id.clone()
        {
            self.open_saved_theme_editor(theme_id, window, cx);
            return;
        }

        self.open_detached_theme_editor(window, cx);
    }

    pub(super) fn edit_saved_theme_here(
        &mut self,
        theme_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_saved_theme_editor(theme_id, window, cx);
    }

    pub(super) fn edit_saved_theme_externally(&mut self, theme_id: String, cx: &mut Context<Self>) {
        let editor_id = saved_theme_editor_id(&theme_id);
        self.open_css_in_preferred_editor_for(Some(theme_id), Some(editor_id), cx);
    }

    pub(super) fn edit_detached_theme_externally(&mut self, cx: &mut Context<Self>) {
        self.open_css_in_preferred_editor_for(None, Some(DETACHED_THEME_EDITOR_ID.to_owned()), cx);
    }

    fn open_saved_theme_editor(
        &mut self,
        theme_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_writable {
            self.settings_notice =
                Some("The CSS editor is unavailable while settings are read-only.".into());
            cx.notify();
            return;
        }
        let editor_id = saved_theme_editor_id(&theme_id);
        if let Some(editor) = self
            .theme_editors
            .get(&editor_id)
            .map(|session| session.editor.clone())
        {
            self.open_workspace_tool_tab(WorkspaceToolTab::ThemeCss(editor_id), window, cx);
            editor.read(cx).focus_handle(cx).focus(window);
            return;
        }

        let Some(theme) = self.settings.theme.saved_theme(&theme_id).cloned() else {
            self.settings_notice = Some("That saved theme no longer exists.".into());
            cx.notify();
            return;
        };
        if let Err(error) = crate::theme::parse_css(&theme.css_source) {
            self.settings_notice =
                Some(format!("Saved theme “{}” is invalid: {error}", theme.name));
            cx.notify();
            return;
        }
        let baseline = theme.css_source.clone();
        let draft_path = theme.draft_path.clone();
        let path = draft_path.clone().or_else(|| theme.source_path.clone());
        let disk_source = path.as_deref().and_then(|path| read_css_theme(path).ok());
        let source = theme
            .draft_source
            .clone()
            .or_else(|| {
                draft_path
                    .as_deref()
                    .and_then(|path| read_css_theme(path).ok())
            })
            .unwrap_or_else(|| baseline.clone());
        let disk_source = trusted_theme_disk_source(
            disk_source,
            theme.draft_disk_source.as_deref(),
            &source,
            &baseline,
            path.as_ref() == theme.source_path.as_ref(),
        );
        self.create_theme_editor_session(
            editor_id,
            Some(theme_id),
            theme.name,
            baseline,
            source,
            path,
            disk_source,
            window,
            cx,
        );
    }

    fn open_detached_theme_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editor_id = DETACHED_THEME_EDITOR_ID.to_owned();
        if let Some(editor) = self
            .theme_editors
            .get(&editor_id)
            .map(|session| session.editor.clone())
        {
            self.open_workspace_tool_tab(WorkspaceToolTab::ThemeCss(editor_id), window, cx);
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
        let theme_name = crate::theme::parse_css(&baseline)
            .map(|theme| theme.name.to_string())
            .unwrap_or_else(|_| "Unsaved theme".to_owned());
        let disk_source = trusted_theme_disk_source(
            disk_source,
            self.settings.theme.draft_disk_source.as_deref(),
            &source,
            &baseline,
            path.as_ref() == self.settings.theme.source_path.as_ref(),
        );
        self.create_theme_editor_session(
            editor_id,
            None,
            theme_name,
            baseline,
            source,
            path,
            disk_source,
            window,
            cx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn create_theme_editor_session(
        &mut self,
        editor_id: String,
        theme_id: Option<String>,
        theme_name: String,
        baseline: String,
        source: String,
        path: Option<PathBuf>,
        disk_source: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
                    .framed(false)
                    .completion_provider(intelligence.clone())
                    .hover_provider(intelligence)
                    .diagnostic_provider(crate::theme::theme_css_diagnostics),
                window,
                cx,
            )
        });
        let editor_settings = self.settings.editor.clone();
        editor.update(cx, |editor, cx| {
            editor.apply_editor_settings(&editor_settings, window, cx);
        });
        let changed_editor_id = editor_id.clone();
        let subscription = cx.subscribe(&editor, move |this, _, event: &InputEvent, cx| {
            if !matches!(event, InputEvent::Change) {
                return;
            }
            this.theme_editor_changed(&changed_editor_id, cx);
        });

        self.theme_editors.insert(
            editor_id.clone(),
            ThemeEditorSession {
                theme_id,
                theme_name,
                editor: editor.clone(),
                path,
                baseline,
                dirty: recovered,
                disk_source,
                validation_task: None,
                persist_task: None,
                _subscription: subscription,
            },
        );
        self.open_workspace_tool_tab(WorkspaceToolTab::ThemeCss(editor_id), window, cx);
        self.settings_notice = recovered
            .then(|| "Your unsaved changes were restored. Save or revert them when ready.".into());
        editor.read(cx).focus_handle(cx).focus(window);
        cx.notify();
    }

    pub(super) fn apply_theme_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor_id) = self
            .workspace_tabs
            .active_theme_editor_id()
            .map(ToOwned::to_owned)
        else {
            return;
        };
        let Some((theme_id, editor, display_name)) =
            self.theme_editors.get(&editor_id).map(|session| {
                (
                    session.theme_id.clone(),
                    session.editor.clone(),
                    session.theme_name.clone(),
                )
            })
        else {
            return;
        };
        let Some(theme_id) = theme_id else {
            self.settings_notice = Some(
                "This editor is not linked to a saved theme. Save it as a new theme instead."
                    .into(),
            );
            cx.notify();
            return;
        };
        let source = editor.read(cx).value(cx).to_string();
        let parsed = match crate::theme::parse_css(&source) {
            Ok(theme) => theme,
            Err(error) => {
                self.settings_notice = Some(format!("Theme was not applied: {error}"));
                cx.notify();
                return;
            }
        };
        if self.settings.theme.saved_theme(&theme_id).is_none() {
            self.settings_notice = Some(
                "This theme is no longer in the library. Save it as a new theme instead.".into(),
            );
            cx.notify();
            return;
        }
        let applies_to_active_theme = self.settings.theme.active_theme_id.as_deref()
            == Some(theme_id.as_str())
            && !self.has_detached_theme_snapshot();
        let mut candidate = self.settings.clone();
        let saved = candidate
            .theme
            .saved_themes
            .iter_mut()
            .find(|saved| saved.id == theme_id)
            .expect("saved theme was checked before writing its source");
        saved.css_source = source.clone();
        saved.source_path = None;
        saved.draft_source = None;
        saved.draft_path = None;
        saved.draft_disk_source = None;
        if applies_to_active_theme {
            candidate.theme.source_path = None;
            candidate.theme.css_source = Some(source.clone());
        }
        match self.commit_settings(candidate, false, cx) {
            Ok(()) => {
                if applies_to_active_theme {
                    crate::theme::apply(parsed, cx);
                    self.refresh_variable_intelligence(cx);
                }
                if let Some(session) = self.theme_editors.get_mut(&editor_id) {
                    session.path = None;
                    session.baseline = source;
                    session.dirty = false;
                    session.disk_source = None;
                    session.persist_task = None;
                }
                self.settings_notice = Some(if applies_to_active_theme {
                    format!("Saved and applied “{display_name}”.")
                } else {
                    format!("Saved “{display_name}”.")
                });
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
        let Some(editor) = self
            .active_theme_editor()
            .map(|session| session.editor.clone())
        else {
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
                                    "Choose a name for this theme. Names may be up to 80 characters.",
                                ),
                        )
                        .child(Input::new(&input_for_dialog)),
                )
        });
        name_input.read(cx).focus_handle(cx).focus(window);
    }

    pub(super) fn open_create_theme_from_template_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_writable {
            self.settings_notice =
                Some("The theme library is read-only because settings could not be loaded.".into());
            cx.notify();
            return;
        }
        if self.has_detached_theme_snapshot() {
            self.settings_notice = Some("Save this theme before creating another.".into());
            cx.notify();
            return;
        }
        if self.has_unscoped_theme_draft() {
            self.settings_notice =
                Some("Save or revert your changes before creating another theme.".into());
            cx.notify();
            return;
        }

        let source = crate::theme::bundled_css().to_owned();
        let parsed_name = crate::theme::parse_css(&source)
            .expect("the bundled CSS theme is validated by the theme test suite")
            .name
            .to_string();
        let default_name =
            next_available_theme_name(&self.settings.theme.saved_themes, &parsed_name);
        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Theme name")
                .default_value(default_name)
        });
        let this = cx.entity().downgrade();
        let input_for_dialog = name_input.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let create_this = this.clone();
            let input_for_create = input_for_dialog.clone();
            let source_for_create = source.clone();
            dialog
                .title("Create theme from template")
                .w(px(460.))
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("Create theme"))
                .on_ok(move |_, _, cx| {
                    let name = input_for_create.read(cx).value().trim().to_owned();
                    if name.is_empty() || name.chars().count() > 80 {
                        return false;
                    }
                    let Some(this) = create_this.upgrade() else {
                        return true;
                    };
                    this.update(cx, |this, cx| {
                        if let Some(name) =
                            this.save_new_theme_source(name, source_for_create.clone(), cx)
                        {
                            this.settings_notice = Some(format!(
                                "Created and selected “{name}” from the default template."
                            ));
                        }
                        cx.notify();
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
                                    "Create a separate saved theme from the documented default CSS. You can edit it from its library row.",
                                ),
                        )
                        .child(Input::new(&input_for_dialog)),
                )
        });
        name_input.read(cx).focus_handle(cx).focus(window);
    }

    fn save_theme_editor_as_new(&mut self, requested_name: String, cx: &mut Context<Self>) {
        let Some(editor_id) = self
            .workspace_tabs
            .active_theme_editor_id()
            .map(ToOwned::to_owned)
        else {
            return;
        };
        let Some(editor) = self
            .theme_editors
            .get(&editor_id)
            .map(|session| session.editor.clone())
        else {
            return;
        };
        let source = editor.read(cx).value(cx).to_string();
        if let Some(name) = self.save_new_theme_source(requested_name, source.clone(), cx) {
            let Some(theme_id) = self.settings.theme.active_theme_id.clone() else {
                return;
            };
            let new_editor_id = saved_theme_editor_id(&theme_id);
            if let Some(mut session) = self.theme_editors.remove(&editor_id) {
                let changed_editor_id = new_editor_id.clone();
                let subscription =
                    cx.subscribe(&session.editor, move |this, _, event: &InputEvent, cx| {
                        if matches!(event, InputEvent::Change) {
                            this.theme_editor_changed(&changed_editor_id, cx);
                        }
                    });
                session.theme_id = Some(theme_id);
                session.theme_name = name.clone();
                session.path = None;
                session.baseline = source;
                session.dirty = false;
                session.disk_source = None;
                session.persist_task = None;
                session._subscription = subscription;
                self.theme_editors.insert(new_editor_id.clone(), session);
                self.workspace_tabs
                    .replace_theme_editor_id(&editor_id, new_editor_id);
            }
            self.settings_notice = Some(format!("Saved and selected “{name}”."));
        }
        cx.notify();
    }

    fn save_new_theme_source(
        &mut self,
        requested_name: String,
        source: String,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        if !self.settings_writable {
            self.settings_notice =
                Some("The theme library is read-only because settings could not be loaded.".into());
            return None;
        }
        let parsed = match crate::theme::parse_css(&source) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.settings_notice = Some(format!("Theme was not saved: {error}"));
                return None;
            }
        };
        let name = next_available_theme_name(&self.settings.theme.saved_themes, &requested_name);
        let saved = SavedTheme::new(name.clone(), source.clone(), None);
        let saved_id = saved.id.clone();
        let mut candidate = self.settings.clone();
        candidate.theme.saved_themes.push(saved);
        candidate.theme.active_theme_id = Some(saved_id);
        candidate.theme.source_path = None;
        candidate.theme.css_source = Some(source);
        candidate.theme.draft_source = None;
        candidate.theme.draft_path = None;
        candidate.theme.draft_disk_source = None;

        match self.commit_settings(candidate, false, cx) {
            Ok(()) => {
                crate::theme::apply(parsed, cx);
                self.refresh_variable_intelligence(cx);
                Some(name)
            }
            Err(error) => {
                self.settings_notice = Some(error);
                None
            }
        }
    }

    pub(super) fn delete_saved_theme(
        &mut self,
        theme_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(theme) = self.settings.theme.saved_theme(&theme_id) else {
            self.settings_notice = Some("That saved theme no longer exists.".into());
            cx.notify();
            return;
        };
        let editor_dirty = self
            .theme_editors
            .values()
            .any(|session| session.theme_id.as_deref() == Some(&theme_id) && session.dirty);
        if editor_dirty || theme.draft_source.is_some() || theme.draft_path.is_some() {
            self.settings_notice =
                Some("Save or revert this theme’s changes before deleting it.".into());
            cx.notify();
            return;
        }
        let name = theme.name.clone();
        let removal_message = saved_theme_removal_message(theme);
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let delete_this = this.clone();
            let theme_id = theme_id.clone();
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
                self.dismiss_theme_editor(&saved_theme_editor_id(theme_id));
                if was_active {
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
        let Some(editor_id) = self
            .workspace_tabs
            .active_theme_editor_id()
            .map(ToOwned::to_owned)
        else {
            return;
        };
        let Some((editor, baseline, theme_id)) =
            self.theme_editors.get(&editor_id).map(|session| {
                (
                    session.editor.clone(),
                    session.baseline.clone(),
                    session.theme_id.clone(),
                )
            })
        else {
            return;
        };
        editor.update(cx, |editor, cx| {
            editor.set_value(baseline.clone(), window, cx);
        });
        let mut candidate = self.settings.clone();
        let source_path = match theme_id.as_deref() {
            Some(theme_id) => {
                let Some(theme) = candidate
                    .theme
                    .saved_themes
                    .iter_mut()
                    .find(|theme| theme.id == theme_id)
                else {
                    return;
                };
                theme.draft_source = None;
                theme.draft_path = None;
                theme.draft_disk_source = None;
                theme.source_path.clone()
            }
            None => {
                candidate.theme.draft_source = None;
                candidate.theme.draft_path = None;
                candidate.theme.draft_disk_source = None;
                candidate.theme.source_path.clone()
            }
        };
        if let Some(session) = self.theme_editors.get_mut(&editor_id) {
            session.path = source_path;
            session.dirty = false;
            session.disk_source = session
                .path
                .as_deref()
                .and_then(|path| read_css_theme(path).ok())
                .filter(|disk| disk == &baseline);
            session.persist_task = None;
        }
        self.settings_notice = Some(match self.commit_settings(candidate, false, cx) {
            Ok(()) => "Reverted your changes.".into(),
            Err(error) => {
                format!(
                    "The editor was reverted, but the saved recovery copy could not be cleared: {error}"
                )
            }
        });
        cx.notify();
    }

    pub(super) fn restore_default_theme_template(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor_id) = self
            .workspace_tabs
            .active_theme_editor_id()
            .map(ToOwned::to_owned)
        else {
            return;
        };
        let Some(editor) = self
            .theme_editors
            .get(&editor_id)
            .map(|session| session.editor.clone())
        else {
            return;
        };
        editor.update(cx, |editor, cx| {
            editor.set_value(crate::theme::bundled_css(), window, cx);
        });
        self.theme_editor_changed(&editor_id, cx);
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
        let Some(editor_id) = self
            .workspace_tabs
            .active_theme_editor_id()
            .map(ToOwned::to_owned)
        else {
            return;
        };
        if self
            .theme_editors
            .get(&editor_id)
            .is_some_and(|session| session.dirty)
        {
            let this = cx.entity().downgrade();
            window.open_dialog(cx, move |dialog, _, cx| {
                let reload_this = this.clone();
                let editor_id = editor_id.clone();
                dialog
                    .title("Replace your unsaved changes?")
                    .w(px(460.))
                    .confirm()
                    .button_props(
                        DialogButtonProps::default()
                            .ok_text("Reload file")
                            .ok_variant(ButtonVariant::Danger),
                    )
                    .on_ok(move |_, window, cx| {
                        if let Some(this) = reload_this.upgrade() {
                            this.update(cx, |this, cx| {
                                this.reload_theme_editor_from_disk_now(
                                    &editor_id,
                                    window,
                                    cx,
                                );
                            });
                        }
                        true
                    })
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                "The latest version of the file will replace your unsaved changes. This cannot be undone.",
                            ),
                    )
            });
            return;
        }
        self.reload_theme_editor_from_disk_now(&editor_id, window, cx);
    }

    fn reload_theme_editor_from_disk_now(
        &mut self,
        editor_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self
            .theme_editors
            .get(editor_id)
            .and_then(|session| session.path.clone())
        else {
            self.settings_notice = Some("This theme has no file to reload.".into());
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
        let Some(editor) = self
            .theme_editors
            .get(editor_id)
            .map(|session| session.editor.clone())
        else {
            return;
        };
        editor.update(cx, |editor, cx| {
            editor.set_value(source.clone(), window, cx);
        });
        if let Some(session) = self.theme_editors.get_mut(editor_id) {
            session.disk_source = Some(source);
        }
        self.theme_editor_changed(editor_id, cx);
        if !self.persist_theme_editor_draft(editor_id, cx) {
            cx.notify();
            return;
        }
        self.settings_notice =
            Some("Reloaded the file into the CSS editor. Review it, then save when ready.".into());
        cx.notify();
    }

    pub(super) fn open_css_in_preferred_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor_id) = self
            .workspace_tabs
            .active_theme_editor_id()
            .map(ToOwned::to_owned)
        else {
            self.settings_notice = Some("Open a theme editor first.".into());
            cx.notify();
            return;
        };
        let theme_id = self
            .theme_editors
            .get(&editor_id)
            .and_then(|session| session.theme_id.clone());
        self.open_css_in_preferred_editor_for(theme_id, Some(editor_id), cx);
    }

    fn open_css_in_preferred_editor_for(
        &mut self,
        theme_id: Option<String>,
        editor_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_writable {
            self.settings_notice = Some(
                "This theme can’t be opened in another app while settings are read-only.".into(),
            );
            cx.notify();
            return;
        }

        let session = editor_id
            .as_deref()
            .and_then(|editor_id| self.theme_editors.get(editor_id));
        let saved_theme = theme_id
            .as_deref()
            .and_then(|theme_id| self.settings.theme.saved_theme(theme_id));
        let baseline = session
            .map(|session| session.baseline.clone())
            .or_else(|| saved_theme.map(|theme| theme.css_source.clone()))
            .unwrap_or_else(|| self.current_theme_source().to_owned());
        let source = session
            .map(|session| session.editor.read(cx).value(cx).to_string())
            .or_else(|| saved_theme.and_then(|theme| theme.draft_source.clone()))
            .or_else(|| {
                theme_id
                    .is_none()
                    .then(|| self.settings.theme.draft_source.clone())
                    .flatten()
            })
            .unwrap_or_else(|| baseline.clone());
        let persisted_path = saved_theme
            .and_then(|theme| theme.source_path.clone())
            .or_else(|| {
                theme_id
                    .is_none()
                    .then(|| self.settings.theme.source_path.clone())
                    .flatten()
            });
        let draft_pending = source != baseline;
        let path = session
            .and_then(|session| session.path.clone())
            .or_else(|| saved_theme.and_then(|theme| theme.draft_path.clone()))
            .or_else(|| {
                theme_id
                    .is_none()
                    .then(|| self.settings.theme.draft_path.clone())
                    .flatten()
            })
            .or_else(|| persisted_path.clone())
            .filter(|path| !(draft_pending && persisted_path.as_ref() == Some(path)));
        let mut path = match path {
            Some(path) => path,
            None => next_available_managed_theme_path(draft_pending),
        };

        if draft_pending {
            let known_disk_source = session
                .and_then(|session| session.disk_source.as_deref())
                .or_else(|| saved_theme.and_then(|theme| theme.draft_disk_source.as_deref()))
                .or_else(|| {
                    theme_id
                        .is_none()
                        .then_some(self.settings.theme.draft_disk_source.as_deref())
                        .flatten()
                });
            match inspect_external_theme_source(&path, known_disk_source, &source) {
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

        let persisted = if let Some(editor_id) = editor_id.as_deref()
            && self.theme_editors.contains_key(editor_id)
        {
            if let Some(session) = self.theme_editors.get_mut(editor_id) {
                session.path = Some(path.clone());
                session.disk_source = Some(source.clone());
            }
            self.persist_theme_editor_draft(editor_id, cx)
        } else {
            let mut candidate = self.settings.clone();
            match theme_id.as_deref() {
                Some(theme_id) => {
                    let Some(theme) = candidate
                        .theme
                        .saved_themes
                        .iter_mut()
                        .find(|theme| theme.id == theme_id)
                    else {
                        self.settings_notice = Some("That saved theme no longer exists.".into());
                        cx.notify();
                        return;
                    };
                    if theme.source_path.as_ref() != Some(&path) {
                        theme.draft_path = Some(path.clone());
                        theme.draft_disk_source = Some(source.clone());
                    }
                    if draft_pending {
                        theme.draft_source = Some(source.clone());
                    }
                }
                None => {
                    if candidate.theme.source_path.as_ref() != Some(&path) {
                        candidate.theme.draft_path = Some(path.clone());
                        candidate.theme.draft_disk_source = Some(source.clone());
                    }
                    if draft_pending {
                        candidate.theme.draft_source = Some(source.clone());
                    }
                }
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
            "Opened {} in your preferred CSS app. Reload the file here before saving changes made there.",
            path.display()
        ));
        cx.notify();
    }

    pub(super) fn close_theme_editor(&mut self, editor_id: &str, cx: &mut Context<Self>) -> bool {
        if !self.persist_theme_editor_draft(editor_id, cx) {
            cx.notify();
            return false;
        }
        let draft_saved = self
            .theme_editors
            .get(editor_id)
            .is_some_and(|session| session.dirty);
        self.dismiss_theme_editor(editor_id);
        self.settings_notice = Some(if draft_saved {
            "CSS editor closed. Your unsaved changes will be restored when you reopen it.".into()
        } else {
            "CSS editor closed.".into()
        });
        cx.notify();
        true
    }

    pub(super) fn has_unscoped_theme_draft(&self) -> bool {
        let unscoped_editor_dirty = self
            .theme_editors
            .values()
            .any(|session| session.theme_id.is_none() && session.dirty);
        let unscoped_saved_draft = (self.settings.theme.active_theme_id.is_none()
            || self.has_detached_theme_snapshot())
            && (self.settings.theme.draft_source.is_some()
                || self.settings.theme.draft_path.is_some());
        unscoped_editor_dirty || unscoped_saved_draft
    }

    pub(super) fn discard_external_theme_draft(&mut self, cx: &mut Context<Self>) {
        let active_theme_id = self.settings.theme.active_theme_id.clone();
        if self
            .theme_editors
            .values()
            .any(|session| session.theme_id == active_theme_id && session.dirty)
            || (active_theme_id.is_none() && self.settings.theme.draft_source.is_some())
        {
            self.settings_notice =
                Some("Save or revert your changes before ignoring the file changes.".into());
            cx.notify();
            return;
        }
        let saved_draft_path = active_theme_id.as_deref().and_then(|theme_id| {
            self.settings
                .theme
                .saved_theme(theme_id)
                .and_then(|theme| theme.draft_path.clone())
        });
        let Some(path) = saved_draft_path
            .clone()
            .or_else(|| self.settings.theme.draft_path.clone())
        else {
            return;
        };
        let mut candidate = self.settings.clone();
        candidate.theme.draft_path = None;
        candidate.theme.draft_disk_source = None;
        if let Some(theme_id) = active_theme_id.as_deref()
            && let Some(theme) = candidate
                .theme
                .saved_themes
                .iter_mut()
                .find(|theme| theme.id == theme_id)
        {
            theme.draft_path = None;
            theme.draft_disk_source = None;
        }
        match self.commit_settings(candidate, false, cx) {
            Ok(()) => {
                let source_path = active_theme_id
                    .as_deref()
                    .and_then(|theme_id| self.settings.theme.saved_theme(theme_id))
                    .and_then(|theme| theme.source_path.clone())
                    .or_else(|| self.settings.theme.source_path.clone());
                for session in self
                    .theme_editors
                    .values_mut()
                    .filter(|session| session.theme_id == active_theme_id)
                {
                    session.path = source_path.clone();
                    session.disk_source = session
                        .path
                        .as_deref()
                        .and_then(|source_path| read_css_theme(source_path).ok());
                }
                self.settings_notice = Some(format!(
                    "Resolved will no longer watch {} for changes. The file was kept.",
                    path.display()
                ));
            }
            Err(error) => self.settings_notice = Some(error),
        }
        cx.notify();
    }

    pub(super) fn discard_saved_theme_external_draft(
        &mut self,
        theme_id: String,
        cx: &mut Context<Self>,
    ) {
        if self
            .theme_editors
            .values()
            .any(|session| session.theme_id.as_deref() == Some(&theme_id) && session.dirty)
        {
            self.settings_notice =
                Some("Save or revert your changes before ignoring the file changes.".into());
            cx.notify();
            return;
        }
        let Some(theme) = self.settings.theme.saved_theme(&theme_id) else {
            self.settings_notice = Some("That saved theme no longer exists.".into());
            cx.notify();
            return;
        };
        let Some(path) = theme.draft_path.clone() else {
            return;
        };
        let source_path = theme.source_path.clone();
        let mut candidate = self.settings.clone();
        let Some(theme) = candidate
            .theme
            .saved_themes
            .iter_mut()
            .find(|theme| theme.id == theme_id)
        else {
            return;
        };
        theme.draft_path = None;
        theme.draft_disk_source = None;
        match self.commit_settings(candidate, false, cx) {
            Ok(()) => {
                for session in self
                    .theme_editors
                    .values_mut()
                    .filter(|session| session.theme_id.as_deref() == Some(&theme_id))
                {
                    session.path = source_path.clone();
                    session.disk_source = session
                        .path
                        .as_deref()
                        .and_then(|source_path| read_css_theme(source_path).ok());
                }
                self.settings_notice = Some(format!(
                    "Resolved will no longer watch {} for changes. The file was kept.",
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
        let editor_ids = self.theme_editors.keys().cloned().collect::<Vec<_>>();
        let theme_saved = editor_ids
            .iter()
            .all(|editor_id| self.persist_theme_editor_draft(editor_id, cx));
        if !theme_saved {
            self.settings_notice = Some("Your theme changes could not be saved.".into());
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

    fn dismiss_theme_editor(&mut self, editor_id: &str) {
        self.theme_editors.remove(editor_id);
        self.workspace_tabs
            .close_tool(&WorkspaceToolTab::ThemeCss(editor_id.to_owned()));
    }

    fn theme_editor_changed(&mut self, editor_id: &str, cx: &mut Context<Self>) {
        let Some((editor, baseline)) = self
            .theme_editors
            .get(editor_id)
            .map(|session| (session.editor.clone(), session.baseline.clone()))
        else {
            return;
        };
        let dirty = editor.read(cx).value(cx).as_ref() != baseline;
        let Some(session) = self.theme_editors.get_mut(editor_id) else {
            return;
        };
        if dirty != session.dirty {
            session.dirty = dirty;
            cx.notify();
        }
        session.validation_task = Some(cx.spawn(async move |this, cx| {
            Timer::after(THEME_EDITOR_VALIDATION_DEBOUNCE).await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |_, cx| cx.notify()).ok();
            }
        }));
        let persist_editor_id = editor_id.to_owned();
        session.persist_task = Some(cx.spawn(async move |this, cx| {
            Timer::after(THEME_EDITOR_PERSIST_DEBOUNCE).await;
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                if let Some(session) = this.theme_editors.get_mut(&persist_editor_id) {
                    session.persist_task = None;
                }
                if !this.persist_theme_editor_draft(&persist_editor_id, cx) {
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn persist_theme_editor_draft(&mut self, editor_id: &str, cx: &mut Context<Self>) -> bool {
        let Some((theme_id, editor, baseline, path, disk_source)) =
            self.theme_editors.get(editor_id).map(|session| {
                (
                    session.theme_id.clone(),
                    session.editor.clone(),
                    session.baseline.clone(),
                    session.path.clone(),
                    session.disk_source.clone(),
                )
            })
        else {
            return true;
        };
        let source = editor.read(cx).value(cx).to_string();
        let dirty = source != baseline;
        if let Some(session) = self.theme_editors.get_mut(editor_id) {
            session.dirty = dirty;
        }
        let mut candidate = self.settings.clone();
        match theme_id.as_deref() {
            Some(theme_id) => {
                let Some(theme) = candidate
                    .theme
                    .saved_themes
                    .iter_mut()
                    .find(|theme| theme.id == theme_id)
                else {
                    self.settings_notice = Some("This theme is no longer in the library.".into());
                    return false;
                };
                let draft_path = path.filter(|path| theme.source_path.as_ref() != Some(path));
                theme.draft_source = dirty.then_some(source.clone());
                theme.draft_path = draft_path.clone();
                theme.draft_disk_source = (dirty || draft_path.is_some())
                    .then(|| disk_source.clone())
                    .flatten();
            }
            None => {
                let draft_path =
                    path.filter(|path| candidate.theme.source_path.as_ref() != Some(path));
                candidate.theme.draft_source = dirty.then_some(source);
                candidate.theme.draft_path = draft_path.clone();
                candidate.theme.draft_disk_source = (dirty || draft_path.is_some())
                    .then_some(disk_source)
                    .flatten();
            }
        }
        if candidate == self.settings {
            return true;
        }
        match self.commit_settings(candidate, false, cx) {
            Ok(()) => true,
            Err(error) => {
                self.settings_notice =
                    Some(format!("Your theme changes could not be saved: {error}"));
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
        let existing_index = existing_index.filter(|index| {
            let theme = &candidate.theme.saved_themes[*index];
            theme.draft_source.is_none()
                && theme.draft_path.is_none()
                && !self
                    .theme_editors
                    .values()
                    .any(|session| session.theme_id.as_deref() == Some(&theme.id) && session.dirty)
        });
        let (theme_id, theme_name, updated_existing) = if let Some(index) = existing_index {
            let saved = &mut candidate.theme.saved_themes[index];
            saved.css_source = source.clone();
            saved.source_path = source_path.clone();
            saved.draft_source = None;
            saved.draft_path = None;
            saved.draft_disk_source = None;
            (saved.id.clone(), saved.name.clone(), true)
        } else {
            let theme_name = next_available_theme_name(&candidate.theme.saved_themes, &parsed_name);
            let saved = SavedTheme::new(theme_name.clone(), source.clone(), source_path.clone());
            let theme_id = saved.id.clone();
            candidate.theme.saved_themes.push(saved);
            (theme_id, theme_name, false)
        };
        candidate.theme.active_theme_id = Some(theme_id.clone());
        candidate.theme.source_path = source_path;
        candidate.theme.css_source = Some(source);
        candidate.theme.draft_source = None;
        candidate.theme.draft_path = None;
        candidate.theme.draft_disk_source = None;
        match self.commit_settings(candidate, false, cx) {
            Ok(()) => {
                if updated_existing {
                    self.dismiss_theme_editor(&saved_theme_editor_id(&theme_id));
                }
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
            "“{}” will be removed. Its file at {} will be kept.",
            theme.name,
            path.display()
        ),
        None => format!(
            "“{}” is saved only in Resolved. Removing it deletes the only saved copy and cannot be undone.",
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
        .join("resolved-theme.css")
}

fn next_available_managed_theme_path(draft: bool) -> PathBuf {
    let base = managed_theme_path();
    let stem = if draft {
        "resolved-theme-draft"
    } else {
        "resolved-theme"
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
        .unwrap_or("resolved-theme.css");
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
                    "Could not prepare the theme file next to {}: {error}",
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
                "saved theme file but could not remove its temporary hard link"
            );
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary_path);
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            return Err(format!(
                "A theme file already exists at {}; it was left unchanged.",
                path.display()
            ));
        }
        return Err(format!(
            "Could not save the theme file to {}: {error}",
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
                "Could not check the theme file at {}: {error}",
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
        "{} changed in another app. Reload the file in Resolved before opening it again. The changes in the other app were kept.",
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

        assert!(error.contains("changed in another app"));
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
        let base = directory.path().join("resolved-theme.css");
        fs::write(&base, "first").unwrap();
        fs::write(directory.path().join("resolved-theme-2.css"), "second").unwrap();

        assert_eq!(
            next_available_theme_path(&base, "resolved-theme"),
            directory.path().join("resolved-theme-3.css")
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
        assert!(message.contains("will be kept"));
        assert!(message.contains(&existing_path.display().to_string()));
    }
}
