use std::{cell::RefCell, collections::HashMap, ops::Range, rc::Rc};

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum SnippetMenuSurface {
    RequestBody,
    PreRequestScript,
    PostResponseScript,
    ResponseBody,
}

impl SnippetMenuSurface {
    const fn source(self) -> SnippetSelectionSource {
        match self {
            Self::RequestBody => SnippetSelectionSource::Request,
            Self::PreRequestScript => SnippetSelectionSource::PreRequest,
            Self::PostResponseScript => SnippetSelectionSource::PostResponse,
            Self::ResponseBody => SnippetSelectionSource::Response,
        }
    }

    const fn area(self) -> SnippetSelectionArea {
        match self {
            Self::RequestBody | Self::ResponseBody => SnippetSelectionArea::Body,
            Self::PreRequestScript | Self::PostResponseScript => SnippetSelectionArea::Script,
        }
    }

    const fn target_category(self) -> Option<SnippetCategory> {
        match self {
            Self::PreRequestScript => Some(SnippetCategory::PreRequest),
            Self::PostResponseScript => Some(SnippetCategory::PostResponse),
            Self::RequestBody | Self::ResponseBody => None,
        }
    }
}

#[derive(Clone)]
struct CapturedEditorSelection {
    range: Range<usize>,
    selection: Option<SnippetSelection>,
}

#[derive(Clone)]
struct SnippetMenuEntry {
    snippet: Snippet,
    available: bool,
    unavailable_message: Option<String>,
    show_category: bool,
}

#[derive(Clone)]
struct SnippetMenuInvocationMarker {
    source_document: SharedString,
    request_tab_id: RequestTabId,
    request_generation: u64,
    response_identity: Option<SnippetResponseIdentity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SnippetResponseIdentity {
    body_address: usize,
    body_len: usize,
}

#[derive(Clone)]
struct SnippetMenuInvocation {
    source_editor: Entity<CodeEditor>,
    source_range: Range<usize>,
    selection: Option<SnippetSelection>,
    marker: SnippetMenuInvocationMarker,
}

struct SnippetSourceEditorOptions {
    category: SnippetCategory,
    kind: SnippetKind,
    source: String,
    read_only: bool,
    variable_catalog: Rc<RefCell<ScriptVariableCatalog>>,
    typescript_service: Option<TypeScriptServiceHandle>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SnippetMenuStaleReason {
    SourceDocument,
    ActiveRequestTab,
    RequestGeneration,
    ResponseState,
}

impl SnippetMenuStaleReason {
    const fn description(self) -> &'static str {
        match self {
            Self::SourceDocument => "the invoking editor changed after its menu opened",
            Self::ActiveRequestTab => "the active request tab changed after its menu opened",
            Self::RequestGeneration => "the request state changed after its menu opened",
            Self::ResponseState => "the response state changed after its menu opened",
        }
    }
}

impl SnippetDraftSnapshot {
    fn empty() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            category: SnippetCategory::PreRequest,
            kind: SnippetKind::Plain,
            source: String::new(),
            requirements: Vec::new(),
        }
    }

    fn from_snippet(snippet: &Snippet) -> Self {
        Self {
            name: snippet.name.clone(),
            description: snippet.description.clone(),
            category: snippet.category,
            kind: snippet.kind,
            source: snippet.source.clone(),
            requirements: snippet.requirements.clone(),
        }
    }

    /// Compare the editable draft using the same text normalization applied
    /// when a snippet is saved. Keeping the raw input intact avoids moving the
    /// caret while still treating harmless surrounding whitespace as saved.
    fn is_equivalent_to(&self, other: &Self) -> bool {
        self.name.trim() == other.name.trim()
            && self.description.trim() == other.description.trim()
            && self.category == other.category
            && self.kind == other.kind
            && self.source == other.source
            && self.requirements == other.requirements
    }
}

impl ApiTester {
    pub(in crate::app) fn create_snippet_editor_session(
        snippets: &[Snippet],
        writable: bool,
        variable_catalog: Rc<RefCell<ScriptVariableCatalog>>,
        typescript_service: Option<TypeScriptServiceHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> SnippetEditorSession {
        let initial = snippets.first().cloned();
        let baseline = initial
            .as_ref()
            .map(SnippetDraftSnapshot::from_snippet)
            .unwrap_or_else(SnippetDraftSnapshot::empty);
        let selected_id = initial.as_ref().map(|snippet| snippet.id.clone());
        let category = baseline.category;
        let kind = baseline.kind;

        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search snippets"));
        let initial_name = baseline.name.clone();
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Snippet name")
                .default_value(initial_name)
        });
        let initial_description = baseline.description.clone();
        let description = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Short description (optional)")
                .default_value(initial_description)
        });
        let editor = Self::new_snippet_source_editor(
            SnippetSourceEditorOptions {
                category,
                kind,
                source: baseline.source.clone(),
                read_only: !writable,
                variable_catalog,
                typescript_service,
            },
            window,
            cx,
        );
        let preview_editor = cx.new(|cx| {
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(CodeLanguage::JavaScript)
                    .placeholder("Run preview to see the JavaScript this snippet will insert")
                    .rows(9)
                    .soft_wrap(false)
                    .read_only(true),
                window,
                cx,
            )
        });

        let search_subscription = cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let name_subscription = cx.subscribe(&name, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.invalidate_snippet_preview();
                cx.notify();
            }
        });
        let description_subscription =
            cx.subscribe(&description, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.invalidate_snippet_preview();
                    cx.notify();
                }
            });
        let (editor_subscription, editor_format_subscription) =
            Self::subscribe_snippet_source_editor(&editor, window, cx);

        SnippetEditorSession {
            search,
            name,
            description,
            editor,
            preview_editor,
            selected_id,
            category,
            kind,
            requirements: baseline.requirements.clone(),
            baseline,
            notice: None,
            notice_is_error: false,
            preview_status: None,
            preview_logs: Vec::new(),
            preview_running: false,
            preview_valid: false,
            preview_generation: 0,
            preview_cancellation: None,
            _input_subscriptions: vec![
                search_subscription,
                name_subscription,
                description_subscription,
            ],
            _editor_subscription: editor_subscription,
            _editor_format_subscription: editor_format_subscription,
        }
    }

    fn new_snippet_source_editor(
        options: SnippetSourceEditorOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<CodeEditor> {
        let SnippetSourceEditorOptions {
            category,
            kind,
            source,
            read_only,
            variable_catalog,
            typescript_service,
        } = options;
        let phase = match category {
            SnippetCategory::PreRequest => SnippetTargetPhase::PreRequest,
            SnippetCategory::PostResponse => SnippetTargetPhase::PostResponse,
        };
        cx.new(|cx| {
            let mut config = CodeEditorConfig::default()
                .language(CodeLanguage::JavaScript)
                .initial_value(source)
                .rows(18)
                .soft_wrap(false)
                .read_only(read_only)
                .format_action(!read_only)
                .placeholder(match kind {
                    SnippetKind::Plain => "JavaScript to add to the selected script",
                    SnippetKind::Executable => {
                        "return `console.log(${JSON.stringify(snippet.selection?.text ?? \"Hello\")})`;"
                    }
                });
            config = match kind {
                SnippetKind::Plain => {
                    let mut intelligence = ScriptCompletionProvider::for_plain_snippet(
                        phase.script_editor_phase(),
                        variable_catalog,
                    );
                    if let Some(service) = typescript_service.clone() {
                        intelligence = intelligence.with_typescript_service(service);
                    }
                    let intelligence = Rc::new(intelligence);
                    let diagnostic_intelligence = Rc::clone(&intelligence);
                    config
                        .completion_provider(intelligence.clone())
                        .hover_provider(intelligence)
                        .async_diagnostic_provider(move |source, cx| {
                            let diagnostics = diagnostic_intelligence.diagnostics_task(source, cx);
                            cx.background_spawn(async move {
                                diagnostics.await.into_iter().map(Into::into).collect()
                            })
                        })
                }
                SnippetKind::Executable => {
                    let mut intelligence = SnippetIntelligenceProvider::new(
                        SnippetEditorContext::ExecutableGenerator(phase),
                    );
                    if let Some(service) = typescript_service.clone() {
                        intelligence = intelligence.with_typescript_service(service);
                    }
                    let intelligence = Rc::new(intelligence);
                    let diagnostic_intelligence = Rc::clone(&intelligence);
                    config
                        .completion_provider(intelligence.clone())
                        .hover_provider(intelligence)
                        .async_diagnostic_provider(move |source, cx| {
                            let diagnostics = diagnostic_intelligence.diagnostics_task(source, cx);
                            cx.background_spawn(async move {
                                diagnostics.await.into_iter().map(Into::into).collect()
                            })
                        })
                }
            };
            CodeEditor::new(config, window, cx)
        })
    }

    fn subscribe_snippet_source_editor(
        editor: &Entity<CodeEditor>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Subscription, Subscription) {
        let change_subscription = cx.subscribe(editor, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.invalidate_snippet_preview();
                cx.notify();
            }
        });
        let format_editor = editor.clone();
        let format_subscription = cx.subscribe_in(
            editor,
            window,
            move |this, _, event: &CodeEditorEvent, window, cx| {
                if matches!(event, CodeEditorEvent::FormatRequested) {
                    this.format_snippet_source_editor(format_editor.clone(), window, cx);
                }
            },
        );
        (change_subscription, format_subscription)
    }

    pub(super) fn replace_snippet_source_editor(
        &mut self,
        category: SnippetCategory,
        kind: SnippetKind,
        source: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.invalidate_snippet_preview();
        let editor = Self::new_snippet_source_editor(
            SnippetSourceEditorOptions {
                category,
                kind,
                source,
                read_only: false,
                variable_catalog: Rc::clone(&self.script_variable_catalog),
                typescript_service: self.typescript_service.clone(),
            },
            window,
            cx,
        );
        let editor_settings = self.settings.editor.clone();
        editor.update(cx, |editor, cx| {
            editor.apply_editor_settings(&editor_settings, window, cx);
        });
        let (subscription, format_subscription) =
            Self::subscribe_snippet_source_editor(&editor, window, cx);
        self.snippet_editor.editor = editor.clone();
        self.snippet_editor._editor_subscription = subscription;
        self.snippet_editor._editor_format_subscription = format_subscription;
        self.snippet_editor.category = category;
        self.snippet_editor.kind = kind;
        self.snippet_editor.preview_running = false;
        self.snippet_editor.preview_valid = false;
        self.snippet_editor.preview_status = None;
        self.snippet_editor.preview_logs.clear();
        editor.read(cx).focus_handle(cx).focus(window);
        cx.notify();
    }

    fn format_snippet_source_editor(
        &mut self,
        editor: Entity<CodeEditor>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if editor.entity_id() != self.snippet_editor.editor.entity_id() {
            return;
        }
        let source = editor.read(cx).value(cx).to_string();
        if source.trim().is_empty() {
            self.set_snippet_notice("The snippet source buffer is empty.", true);
            cx.notify();
            return;
        }

        // dprint's JavaScript parser deliberately accepts `return` outside a
        // function, so executable generator bodies can be formatted directly.
        // Avoiding a synthetic wrapper also preserves raw template-literal
        // indentation byte-for-byte.
        let formatted = match format_snippet_source(&source, &self.settings.formatter) {
            Ok(formatted) => formatted,
            Err(message) => {
                self.set_snippet_notice(message, true);
                cx.notify();
                return;
            }
        };
        if formatted == source {
            self.set_snippet_notice("The snippet source is already formatted.", false);
            cx.notify();
            return;
        }

        let input = editor.read(cx).input_state();
        input.update(cx, |input, cx| {
            let original_cursor = input.cursor();
            let full_range = 0..source.encode_utf16().count();
            EntityInputHandler::replace_text_in_range(
                input,
                Some(full_range),
                &formatted,
                window,
                cx,
            );

            let mut restored_offset = original_cursor.min(formatted.len());
            while !formatted.is_char_boundary(restored_offset) {
                restored_offset = restored_offset.saturating_sub(1);
            }
            let cursor = input.text().offset_to_position(restored_offset);
            input.set_cursor_position(cursor, window, cx);
        });
        self.invalidate_snippet_preview();
        self.set_snippet_notice("Formatted snippet JavaScript.", false);
        cx.notify();
    }

    fn snippet_draft_snapshot(&self, cx: &App) -> SnippetDraftSnapshot {
        SnippetDraftSnapshot {
            name: self.snippet_editor.name.read(cx).value().to_string(),
            description: self.snippet_editor.description.read(cx).value().to_string(),
            category: self.snippet_editor.category,
            kind: self.snippet_editor.kind,
            source: self.snippet_editor.editor.read(cx).value(cx).to_string(),
            requirements: self.snippet_editor.requirements.clone(),
        }
    }

    pub(super) fn snippet_editor_is_dirty(&self, cx: &App) -> bool {
        snippet_draft_is_dirty(
            &self.snippet_draft_snapshot(cx),
            &self.snippet_editor.baseline,
        )
    }

    fn set_snippet_notice(&mut self, message: impl Into<String>, is_error: bool) {
        self.snippet_editor.notice = Some(message.into());
        self.snippet_editor.notice_is_error = is_error;
    }

    pub(super) fn set_snippet_category(
        &mut self,
        category: SnippetCategory,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.snippet_editor.category == category {
            return;
        }
        if category == SnippetCategory::PreRequest {
            self.snippet_editor
                .requirements
                .retain(|requirement| *requirement != SnippetRequirement::HasResponse);
        }
        let source = self.snippet_editor.editor.read(cx).value(cx).to_string();
        self.replace_snippet_source_editor(category, self.snippet_editor.kind, source, window, cx);
    }

    pub(super) fn set_snippet_kind(
        &mut self,
        kind: SnippetKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.snippet_editor.kind == kind {
            return;
        }
        let source = self.snippet_editor.editor.read(cx).value(cx).to_string();
        self.replace_snippet_source_editor(self.snippet_editor.category, kind, source, window, cx);
    }

    pub(super) fn toggle_snippet_requirement(
        &mut self,
        requirement: SnippetRequirement,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        self.snippet_editor
            .requirements
            .retain(|current| *current != requirement && *current != SnippetRequirement::Always);
        if enabled {
            self.snippet_editor.requirements.push(requirement);
        }
        self.invalidate_snippet_preview();
        cx.notify();
    }

    fn current_snippet_definition(&self, cx: &App, preview: bool) -> Result<Snippet, String> {
        let snapshot = self.snippet_draft_snapshot(cx);
        let name = snapshot.name.trim();
        let mut snippet = if let Some(id) = self.snippet_editor.selected_id.as_deref() {
            self.snippets
                .iter()
                .find(|snippet| snippet.id == id)
                .cloned()
                .ok_or_else(|| "This snippet no longer exists.".to_owned())?
        } else {
            Snippet::new(
                if name.is_empty() && preview {
                    "Untitled snippet"
                } else {
                    name
                },
                snapshot.category,
                snapshot.kind,
            )
            .map_err(|error| error.to_string())?
        };
        snippet.name = if name.is_empty() && preview {
            "Untitled snippet".to_owned()
        } else {
            name.to_owned()
        };
        snippet.description = snapshot.description.trim().to_owned();
        snippet.category = snapshot.category;
        snippet.kind = snapshot.kind;
        snippet.source = snapshot.source;
        snippet.requirements = snapshot.requirements;
        snippet.updated_at = Utc::now();
        snippet.normalize();
        snippet.validate().map_err(|error| error.to_string())?;
        Ok(snippet)
    }

    pub(super) fn save_snippet(&mut self, cx: &mut Context<Self>) {
        let snippet = match self.current_snippet_definition(cx, false) {
            Ok(snippet) => snippet,
            Err(error) => {
                self.set_snippet_notice(format!("Snippet was not saved: {error}"), true);
                cx.notify();
                return;
            }
        };
        if let Some(index) = self
            .snippets
            .iter()
            .position(|current| current.id == snippet.id)
        {
            self.snippets[index] = snippet.clone();
        } else {
            self.snippets.push(snippet.clone());
        }
        SNIPPET_LIST_CACHE.with(|cache| cache.borrow_mut().invalidate());
        match self.persist_snippets() {
            Ok(()) => {
                self.snippet_editor.selected_id = Some(snippet.id.clone());
                self.snippet_editor.baseline = SnippetDraftSnapshot::from_snippet(&snippet);
                self.set_snippet_notice(format!("Saved ‘{}’.", snippet.name), false);
            }
            Err(error) => self.set_snippet_notice(error, true),
        }
        cx.notify();
    }

    /// Persist a valid dirty snippet before window/app shutdown. Returning
    /// `false` vetoes the close path and leaves the validation/storage error
    /// visible in the Snippets workspace.
    pub(super) fn flush_snippet_editor(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.snippet_editor_is_dirty(cx) {
            return true;
        }
        self.save_snippet(cx);
        let saved = !self.snippet_editor_is_dirty(cx);
        if !saved {
            let reason = self
                .snippet_editor
                .notice
                .clone()
                .unwrap_or_else(|| "The snippet draft could not be saved.".to_owned());
            self.set_snippet_notice(format!("Close cancelled. {reason}"), true);
            self.dismiss_template_variable_popover();
            self.hide_preview(cx);
            self.workspace_tabs.open_tool(WorkspaceToolTab::Snippets);
            cx.notify();
        }
        saved
    }

    pub(super) fn duplicate_snippet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = match self.current_snippet_definition(cx, true) {
            Ok(snippet) => snippet,
            Err(error) => {
                self.set_snippet_notice(format!("Snippet could not be duplicated: {error}"), true);
                cx.notify();
                return;
            }
        };
        let name = unique_snippet_name(&source.name, source.category, &self.snippets);
        let mut duplicate = match Snippet::new(name, source.category, source.kind) {
            Ok(snippet) => snippet,
            Err(error) => {
                self.set_snippet_notice(format!("Snippet could not be duplicated: {error}"), true);
                cx.notify();
                return;
            }
        };
        duplicate.description = source.description;
        duplicate.source = source.source;
        duplicate.requirements = source.requirements;
        duplicate.output_language = source.output_language;

        self.snippets.push(duplicate.clone());
        SNIPPET_LIST_CACHE.with(|cache| cache.borrow_mut().invalidate());
        match self.persist_snippets() {
            Ok(()) => {
                self.load_snippet_now(duplicate, window, cx);
                self.set_snippet_notice("Snippet duplicated.", false);
            }
            Err(error) => {
                self.set_snippet_notice(error, true);
                cx.notify();
            }
        }
    }

    pub(super) fn request_delete_snippet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.snippet_editor.selected_id.clone() else {
            return;
        };
        let Some(name) = self
            .snippets
            .iter()
            .find(|snippet| snippet.id == id)
            .map(|snippet| snippet.name.clone())
        else {
            return;
        };
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let delete_this = this.clone();
            let id = id.clone();
            dialog
                .title("Delete snippet?")
                .w(px(440.))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete snippet")
                        .ok_variant(ButtonVariant::Danger),
                )
                .on_ok(move |_, window, cx| {
                    if let Some(this) = delete_this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.delete_snippet_now(&id, window, cx);
                        });
                    }
                    true
                })
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "‘{name}’ will be removed from every Snippets menu."
                        )),
                )
        });
    }

    fn delete_snippet_now(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self
            .snippets
            .iter()
            .position(|snippet| snippet.id == id)
        else {
            return;
        };
        self.snippets.remove(index);
        SNIPPET_LIST_CACHE.with(|cache| cache.borrow_mut().invalidate());
        match self.persist_snippets() {
            Ok(()) => {
                if let Some(next) = self
                    .snippets
                    .get(index.min(self.snippets.len().saturating_sub(1)))
                    .cloned()
                {
                    self.load_snippet_now(next, window, cx);
                } else {
                    self.new_snippet_now(window, cx);
                }
                self.set_snippet_notice("Snippet deleted.", false);
            }
            Err(error) => self.set_snippet_notice(error, true),
        }
        cx.notify();
    }

    pub(super) fn request_new_snippet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.snippet_editor_is_dirty(cx) {
            let this = cx.entity().downgrade();
            window.open_dialog(cx, move |dialog, _, _| {
                let discard_this = this.clone();
                dialog
                    .title("Discard snippet changes?")
                    .w(px(430.))
                    .confirm()
                    .button_props(
                        DialogButtonProps::default()
                            .ok_text("Discard and create")
                            .ok_variant(ButtonVariant::Danger),
                    )
                    .on_ok(move |_, window, cx| {
                        if let Some(this) = discard_this.upgrade() {
                            this.update(cx, |this, cx| this.new_snippet_now(window, cx));
                        }
                        true
                    })
                    .child("Your current unsaved snippet edits will be discarded.")
            });
            return;
        }
        self.new_snippet_now(window, cx);
    }

    fn new_snippet_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.snippet_editor
            .name
            .update(cx, |input, cx| input.set_value(String::new(), window, cx));
        self.snippet_editor
            .description
            .update(cx, |input, cx| input.set_value(String::new(), window, cx));
        self.snippet_editor.selected_id = None;
        self.snippet_editor.requirements.clear();
        self.snippet_editor.baseline = SnippetDraftSnapshot::empty();
        self.snippet_editor.notice = None;
        self.replace_snippet_source_editor(
            SnippetCategory::PreRequest,
            SnippetKind::Plain,
            String::new(),
            window,
            cx,
        );
        self.snippet_editor
            .name
            .read(cx)
            .focus_handle(cx)
            .focus(window);
    }

    pub(super) fn request_select_snippet(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.snippet_editor.selected_id.as_deref() == Some(&id) {
            return;
        }
        let Some(snippet) = self
            .snippets
            .iter()
            .find(|snippet| snippet.id == id)
            .cloned()
        else {
            return;
        };
        if self.snippet_editor_is_dirty(cx) {
            let this = cx.entity().downgrade();
            window.open_dialog(cx, move |dialog, _, _| {
                let select_this = this.clone();
                let snippet = snippet.clone();
                dialog
                    .title("Discard snippet changes?")
                    .w(px(430.))
                    .confirm()
                    .button_props(
                        DialogButtonProps::default()
                            .ok_text("Discard and switch")
                            .ok_variant(ButtonVariant::Danger),
                    )
                    .on_ok(move |_, window, cx| {
                        if let Some(this) = select_this.upgrade() {
                            this.update(cx, |this, cx| {
                                this.load_snippet_now(snippet.clone(), window, cx);
                            });
                        }
                        true
                    })
                    .child("Your current unsaved snippet edits will be discarded.")
            });
            return;
        }
        self.load_snippet_now(snippet, window, cx);
    }

    fn load_snippet_now(&mut self, snippet: Snippet, window: &mut Window, cx: &mut Context<Self>) {
        self.snippet_editor.name.update(cx, |input, cx| {
            input.set_value(snippet.name.clone(), window, cx)
        });
        self.snippet_editor.description.update(cx, |input, cx| {
            input.set_value(snippet.description.clone(), window, cx)
        });
        self.snippet_editor.selected_id = Some(snippet.id.clone());
        self.snippet_editor.requirements = snippet.requirements.clone();
        self.snippet_editor.baseline = SnippetDraftSnapshot::from_snippet(&snippet);
        self.snippet_editor.notice = None;
        self.replace_snippet_source_editor(
            snippet.category,
            snippet.kind,
            snippet.source,
            window,
            cx,
        );
    }
}

fn snippet_draft_is_dirty(
    draft: &SnippetDraftSnapshot,
    baseline: &SnippetDraftSnapshot,
) -> bool {
    !draft.is_equivalent_to(baseline)
}

fn unique_snippet_name(
    source_name: &str,
    category: SnippetCategory,
    snippets: &[Snippet],
) -> String {
    let names = snippets
        .iter()
        .filter(|snippet| snippet.category == category)
        .map(|snippet| snippet.name.to_lowercase())
        .collect::<BTreeSet<_>>();
    let mut copy_index = 1_usize;
    loop {
        let suffix = if copy_index == 1 {
            " copy".to_owned()
        } else {
            format!(" copy {copy_index}")
        };
        let candidate = bounded_snippet_name(source_name, &suffix);
        if !names.contains(&candidate.to_lowercase()) {
            return candidate;
        }
        copy_index = copy_index.saturating_add(1);
    }
}

fn bounded_snippet_name(base: &str, suffix: &str) -> String {
    debug_assert!(suffix.len() < MAX_SNIPPET_NAME_BYTES);
    let mut boundary = (MAX_SNIPPET_NAME_BYTES - suffix.len()).min(base.len());
    while boundary > 0 && !base.is_char_boundary(boundary) {
        boundary -= 1;
    }
    format!("{}{}", &base[..boundary], suffix)
}

fn response_identity(response: Option<&ResponseData>) -> Option<SnippetResponseIdentity> {
    response.map(|response| SnippetResponseIdentity {
        body_address: response.body.as_ptr() as usize,
        body_len: response.body.len(),
    })
}

impl SnippetMenuInvocationMarker {
    fn stale_reason(
        &self,
        current_source_document: &str,
        current_request_tab_id: &RequestTabId,
        current_request_generation: u64,
        current_response: Option<&ResponseData>,
    ) -> Option<SnippetMenuStaleReason> {
        if self.source_document.as_ref() != current_source_document {
            return Some(SnippetMenuStaleReason::SourceDocument);
        }
        if &self.request_tab_id != current_request_tab_id {
            return Some(SnippetMenuStaleReason::ActiveRequestTab);
        }
        if self.request_generation != current_request_generation {
            return Some(SnippetMenuStaleReason::RequestGeneration);
        }
        if self.response_identity != response_identity(current_response) {
            return Some(SnippetMenuStaleReason::ResponseState);
        }
        None
    }
}

fn utf16_offset_to_byte_index(value: &str, target: usize) -> Option<usize> {
    let mut utf16_offset = 0_usize;
    for (byte_index, character) in value.char_indices() {
        if utf16_offset == target {
            return Some(byte_index);
        }
        utf16_offset = utf16_offset.checked_add(character.len_utf16())?;
        if utf16_offset > target {
            return None;
        }
    }
    (utf16_offset == target).then_some(value.len())
}

fn script_source_bytes_after_replacement(
    source: &str,
    range: Range<usize>,
    replacement: &str,
) -> Option<usize> {
    if range.start > range.end {
        return None;
    }
    let start = utf16_offset_to_byte_index(source, range.start)?;
    let end = utf16_offset_to_byte_index(source, range.end)?;
    source
        .len()
        .checked_sub(end.checked_sub(start)?)?
        .checked_add(replacement.len())
}

impl ApiTester {
    fn invalidate_snippet_preview(&mut self) {
        if let Some(cancellation) = self.snippet_editor.preview_cancellation.take() {
            cancellation.cancel();
        }
        self.snippet_editor.preview_generation =
            self.snippet_editor.preview_generation.wrapping_add(1);
        self.snippet_editor.preview_running = false;
        self.snippet_editor.preview_status = None;
        self.snippet_editor.preview_logs.clear();
    }

    fn snippet_invocation_context(
        &self,
        category: SnippetCategory,
        selection: Option<SnippetSelection>,
        cx: &App,
    ) -> crate::core::SnippetInvocationContext {
        let current_request = self.draft(cx);
        let request = if category == SnippetCategory::PostResponse {
            self.response_request.as_ref().unwrap_or(&current_request)
        } else {
            &current_request
        };
        let mut context = crate::core::SnippetInvocationContext::new(category, request);
        if category == SnippetCategory::PostResponse
            && let Some(response) = self.response.as_ref()
        {
            context = context.with_response(response);
        }
        if let Some(selection) = selection {
            context = context.with_selection(selection);
        }
        let active_environment_secrets = self
            .workspace
            .active_environment()
            .into_iter()
            .flat_map(|environment| environment.variables.iter())
            .filter(|variable| variable.enabled && variable.secret)
            .map(|variable| variable.value.as_str());
        let response_secrets = (category == SnippetCategory::PostResponse)
            .then_some(self.response_sensitive_values.iter().map(String::as_str))
            .into_iter()
            .flatten();
        context.with_sensitive_values(active_environment_secrets.chain(response_secrets))
    }

    fn preview_selection(
        &self,
        category: SnippetCategory,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<SnippetSelection> {
        let candidates = match category {
            SnippetCategory::PreRequest => vec![
                (self.body.clone(), SnippetMenuSurface::RequestBody),
                (
                    self.pre_request_script.clone(),
                    SnippetMenuSurface::PreRequestScript,
                ),
                (
                    self.response_editor.clone(),
                    SnippetMenuSurface::ResponseBody,
                ),
            ],
            SnippetCategory::PostResponse => vec![
                (
                    self.response_editor.clone(),
                    SnippetMenuSurface::ResponseBody,
                ),
                (self.body.clone(), SnippetMenuSurface::RequestBody),
                (
                    self.post_response_script.clone(),
                    SnippetMenuSurface::PostResponseScript,
                ),
            ],
        };
        candidates.into_iter().find_map(|(editor, surface)| {
            let language = editor.read(cx).language().clone();
            let (content_type, json) = self.snippet_surface_content(surface, &language);
            capture_editor_selection(&editor, surface, content_type, json, window, cx).selection
        })
    }

    fn snippet_surface_content(
        &self,
        surface: SnippetMenuSurface,
        language: &CodeLanguage,
    ) -> (Option<String>, bool) {
        match surface {
            SnippetMenuSurface::RequestBody => {
                let json = self.raw_body_language == RawBodyLanguage::Json;
                (Some(self.raw_body_language.content_type().to_owned()), json)
            }
            SnippetMenuSurface::ResponseBody => {
                let content_type = self
                    .response
                    .as_ref()
                    .and_then(|response| response.content_type.clone());
                let json = content_type
                    .as_deref()
                    .is_some_and(|value| value.to_ascii_lowercase().contains("json"))
                    || matches!(language, CodeLanguage::Json);
                (content_type, json)
            }
            SnippetMenuSurface::PreRequestScript | SnippetMenuSurface::PostResponseScript => {
                (Some("application/javascript".to_owned()), false)
            }
        }
    }

    pub(super) fn run_snippet_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let snippet = match self.current_snippet_definition(cx, true) {
            Ok(snippet) => snippet,
            Err(error) => {
                self.invalidate_snippet_preview();
                self.set_snippet_notice(format!("Preview could not run: {error}"), true);
                cx.notify();
                return;
            }
        };
        let selection = self.preview_selection(snippet.category, window, cx);
        let context = self.snippet_invocation_context(snippet.category, selection, cx);
        if let Some(cancellation) = self.snippet_editor.preview_cancellation.take() {
            cancellation.cancel();
        }
        let cancellation = SnippetCancellation::new();
        self.snippet_editor.preview_generation =
            self.snippet_editor.preview_generation.wrapping_add(1);
        let generation = self.snippet_editor.preview_generation;
        self.snippet_editor.preview_cancellation = Some(cancellation.clone());
        self.snippet_editor.preview_running = true;
        self.snippet_editor.preview_valid = false;
        self.snippet_editor.preview_status = Some("Generating preview…".to_owned());
        self.snippet_editor.preview_logs.clear();
        self.snippet_editor.notice = None;

        let task = self
            .runtime
            .spawn_blocking(move || generate_snippet(&snippet, &context, &cancellation));
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.snippet_editor.preview_generation != generation {
                    return;
                }
                this.snippet_editor.preview_cancellation = None;
                this.snippet_editor.preview_running = false;
                match result {
                    Ok(Ok(generated)) => {
                        this.snippet_editor.preview_logs = generated.report.logs;
                        this.snippet_editor.preview_editor.update(cx, |editor, cx| {
                            editor.set_value(generated.text, window, cx);
                        });
                        this.snippet_editor.preview_valid = true;
                        this.snippet_editor.preview_status = Some("Preview ready".to_owned());
                    }
                    Ok(Err(error)) => {
                        this.snippet_editor.preview_valid = false;
                        let message = error.to_string();
                        this.snippet_editor.preview_logs = error.report.logs;
                        this.snippet_editor.preview_status = Some(message);
                    }
                    Err(error) => {
                        this.snippet_editor.preview_valid = false;
                        this.snippet_editor.preview_status =
                            Some(format!("Snippet preview task failed: {error}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn copy_snippet_preview(&mut self, cx: &mut Context<Self>) {
        if !self.snippet_editor.preview_valid {
            return;
        }
        let text = self
            .snippet_editor
            .preview_editor
            .read(cx)
            .value(cx)
            .to_string();
        if text.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.set_snippet_notice("Preview copied to the clipboard.", false);
        cx.notify();
    }

    fn snippet_menu_stale_reason(
        &self,
        marker: &SnippetMenuInvocationMarker,
        source_editor: &Entity<CodeEditor>,
        cx: &App,
    ) -> Option<SnippetMenuStaleReason> {
        let current_source_document = source_editor.read(cx).value(cx);
        marker.stale_reason(
            current_source_document.as_ref(),
            self.request_tabs.active_tab_id(),
            self.request_generation,
            self.response.as_ref(),
        )
    }

    fn apply_snippet_from_menu(
        &mut self,
        snippet_id: String,
        invocation: SnippetMenuInvocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SnippetMenuInvocation {
            source_editor,
            source_range,
            selection,
            marker,
        } = invocation;
        let Some(snippet) = self
            .snippets
            .iter()
            .find(|snippet| snippet.id == snippet_id)
            .cloned()
        else {
            self.request_notice = Some("That snippet no longer exists.".to_owned());
            cx.notify();
            return;
        };
        if let Some(reason) = self.snippet_menu_stale_reason(&marker, &source_editor, cx) {
            self.request_notice = Some(format!(
                "‘{}’ was not inserted because {}.",
                snippet.name,
                reason.description()
            ));
            cx.notify();
            return;
        }
        let target = match snippet.category {
            SnippetCategory::PreRequest => self.pre_request_script.clone(),
            SnippetCategory::PostResponse => self.post_response_script.clone(),
        };
        let same_editor = source_editor.entity_id() == target.entity_id();
        let expected_source = target.read(cx).value(cx).to_string();
        let destination_range = if same_editor {
            source_range
        } else {
            let end = expected_source.encode_utf16().count();
            end..end
        };
        let context = self.snippet_invocation_context(snippet.category, selection, cx);

        if let Some(cancellation) = self.snippet_apply_cancellation.take() {
            cancellation.cancel();
        }
        let cancellation = SnippetCancellation::new();
        self.snippet_apply_generation = self.snippet_apply_generation.wrapping_add(1);
        let generation = self.snippet_apply_generation;
        self.snippet_apply_cancellation = Some(cancellation.clone());
        self.request_notice = Some(format!("Generating ‘{}’…", snippet.name));
        let snippet_name = snippet.name.clone();
        let category = snippet.category;
        let task = self
            .runtime
            .spawn_blocking(move || generate_snippet(&snippet, &context, &cancellation));
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.snippet_apply_generation != generation {
                    return;
                }
                this.snippet_apply_cancellation = None;
                let generated = match result {
                    Ok(Ok(generated)) => generated,
                    Ok(Err(error)) => {
                        this.request_notice = Some(error.to_string());
                        cx.notify();
                        return;
                    }
                    Err(error) => {
                        this.request_notice =
                            Some(format!("Snippet generation task failed: {error}"));
                        cx.notify();
                        return;
                    }
                };
                if let Some(reason) =
                    this.snippet_menu_stale_reason(&marker, &source_editor, cx)
                {
                    this.request_notice = Some(format!(
                        "‘{snippet_name}’ was not inserted because {}.",
                        reason.description()
                    ));
                    cx.notify();
                    return;
                }
                if target.read(cx).value(cx).as_ref() != expected_source {
                    this.request_notice = Some(format!(
                        "‘{snippet_name}’ was not inserted because the target script changed while it was generating."
                    ));
                    cx.notify();
                    return;
                }
                if generated.text.is_empty() && (!same_editor || destination_range.is_empty()) {
                    this.request_notice =
                        Some(format!("‘{snippet_name}’ generated no code, so nothing was inserted."));
                    cx.notify();
                    return;
                }

                let separator =
                    if same_editor || expected_source.is_empty() || expected_source.ends_with('\n') {
                        ""
                    } else {
                        "\n"
                };
                let inserted = format!("{separator}{}", generated.text);
                let Some(resulting_source_bytes) = script_source_bytes_after_replacement(
                    &expected_source,
                    destination_range.clone(),
                    &inserted,
                ) else {
                    this.request_notice = Some(format!(
                        "‘{snippet_name}’ was not inserted because its destination range is no longer valid."
                    ));
                    cx.notify();
                    return;
                };
                if resulting_source_bytes > crate::core::MAX_SCRIPT_SOURCE_BYTES {
                    this.request_notice = Some(format!(
                        "‘{snippet_name}’ was not inserted because the resulting {} script would be {resulting_source_bytes} bytes; the limit is {} bytes.",
                        category.label(),
                        crate::core::MAX_SCRIPT_SOURCE_BYTES
                    ));
                    cx.notify();
                    return;
                }
                let insertion_start = destination_range.start;
                let preferred_cursor = generated
                    .cursor
                    .unwrap_or_else(|| generated.text.encode_utf16().count());
                let cursor_utf16 = insertion_start
                    + separator.encode_utf16().count()
                    + preferred_cursor;
                let input = target.read(cx).input_state();
                input.update(cx, |input, cx| {
                    EntityInputHandler::replace_text_in_range(
                        input,
                        Some(destination_range.clone()),
                        &inserted,
                        window,
                        cx,
                    );
                    let cursor_utf16 = cursor_utf16.min(input.value().encode_utf16().count());
                    let offset = input.text().offset_utf16_to_offset(cursor_utf16);
                    let point = input.text().offset_to_position(offset);
                    input.set_cursor_position(point, window, cx);
                });
                this.activate_request_workspace(SidebarTab::Collections, window, cx);
                this.request_pane = match category {
                    SnippetCategory::PreRequest => RequestPane::PreRequest,
                    SnippetCategory::PostResponse => RequestPane::PostResponse,
                };
                target.read(cx).focus_handle(cx).focus(window);
                let log_suffix = (!generated.report.logs.is_empty()).then(|| {
                    format!(" · {} generator log(s)", generated.report.logs.len())
                });
                this.request_notice = Some(format!(
                    "Inserted ‘{snippet_name}’ into {}{}.",
                    category.label(),
                    log_suffix.unwrap_or_default()
                ));
                cx.notify();
            });
        })
        .detach();
    }
}

fn capture_editor_selection<C>(
    editor: &Entity<CodeEditor>,
    surface: SnippetMenuSurface,
    content_type: Option<String>,
    json: bool,
    window: &mut Window,
    cx: &mut Context<C>,
) -> CapturedEditorSelection {
    let input = editor.read(cx).input_state();
    let (range, document, selected) = input.update(cx, |input, cx| {
        let Some(selection) = EntityInputHandler::selected_text_range(input, true, window, cx)
        else {
            return (0..0, None, None);
        };
        if selection.range.is_empty() {
            return (selection.range, None, None);
        }
        let mut adjusted = None;
        let selected = EntityInputHandler::text_for_range(
            input,
            selection.range.clone(),
            &mut adjusted,
            window,
            cx,
        )
        .unwrap_or_default();
        let document = json.then(|| input.value());
        (selection.range, document, Some(selected))
    });
    captured_editor_selection(
        range,
        document.as_ref().map(AsRef::as_ref).unwrap_or_default(),
        selected.as_deref().unwrap_or_default(),
        surface,
        content_type,
        json,
    )
}

fn captured_editor_selection(
    range: Range<usize>,
    document: &str,
    selected: &str,
    surface: SnippetMenuSurface,
    content_type: Option<String>,
    json: bool,
) -> CapturedEditorSelection {
    if range.is_empty() {
        return CapturedEditorSelection {
            range,
            selection: None,
        };
    }
    let snippet_range = SnippetTextRange::new(range.start, range.end);
    let selection = if json {
        SnippetSelection::from_json_document(
            surface.source(),
            surface.area(),
            document,
            snippet_range,
            content_type.clone(),
        )
        .or_else(|_| {
            SnippetSelection::new(
                surface.source(),
                surface.area(),
                selected,
                snippet_range,
                content_type,
            )
        })
    } else {
        SnippetSelection::new(
            surface.source(),
            surface.area(),
            selected,
            snippet_range,
            content_type,
        )
    }
    .ok();
    CapturedEditorSelection { range, selection }
}

fn current_surface_selection(
    surface: SnippetMenuSurface,
    has_current_response: bool,
    selection: Option<SnippetSelection>,
) -> Option<SnippetSelection> {
    if surface == SnippetMenuSurface::ResponseBody && !has_current_response {
        None
    } else {
        selection
    }
}

pub(in crate::app) fn snippet_context_menu_builder(
    owner: gpui::WeakEntity<ApiTester>,
    surface: SnippetMenuSurface,
) -> CodeEditorContextMenuBuilder {
    Rc::new(
        move |menu, editor_context: CodeEditorContextMenuContext, window, cx| {
            let Some(owner_entity) = owner.upgrade() else {
                return menu;
            };
            let source_editor = editor_context.editor.clone();
            let (captured, entries, marker) = {
                let state = owner_entity.read(cx);
                let (content_type, json) =
                    state.snippet_surface_content(surface, &editor_context.language);
                let mut captured = captured_editor_selection(
                    editor_context.range,
                    editor_context.document.as_ref(),
                    &editor_context.selected_text,
                    surface,
                    content_type,
                    json,
                );
                // A response editor can retain its old rendered text while a
                // new request is in flight. Never expose that stale text as a
                // current response selection.
                captured.selection = current_surface_selection(
                    surface,
                    state.response.is_some(),
                    captured.selection,
                );
                let response = state.response.as_ref();
                let entries = state
                    .snippets
                    .iter()
                    .filter(|snippet| {
                        surface
                            .target_category()
                            .is_none_or(|category| snippet.category == category)
                    })
                    .cloned()
                    .map(|snippet| {
                        // Menu availability depends only on phase, response
                        // presence, and selection metadata. Avoid building the
                        // full invocation context here because doing so would
                        // synchronously re-read the editor currently handling the
                        // right click.
                        let empty_request = RequestDraft::default();
                        let mut context = crate::core::SnippetInvocationContext::new(
                            snippet.category,
                            &empty_request,
                        );
                        if snippet.category == SnippetCategory::PostResponse
                            && let Some(response) = response
                        {
                            context = context.with_response(response);
                        }
                        if let Some(selection) = captured.selection.clone() {
                            context = context.with_selection(selection);
                        }
                        let availability = snippet.availability(&context);
                        SnippetMenuEntry {
                            snippet,
                            available: availability.is_available(),
                            unavailable_message: availability.message(),
                            show_category: surface.target_category().is_none(),
                        }
                    })
                    .collect::<Vec<_>>();
                let marker = SnippetMenuInvocationMarker {
                    source_document: editor_context.document.clone(),
                    request_tab_id: state.request_tabs.active_tab_id().clone(),
                    request_generation: state.request_generation,
                    response_identity: response_identity(state.response.as_ref()),
                };
                (captured, entries, marker)
            };
            let menu_owner = owner.clone();
            let source_range = captured.range.clone();
            let source_selection = captured.selection.clone();
            menu.submenu("Snippets", window, cx, move |mut submenu, _, _| {
                submenu = submenu.min_w(px(250.)).max_h(px(440.)).scrollable(true);
                if entries.is_empty() {
                    submenu =
                        submenu.item(PopupMenuItem::new("No snippets available").disabled(true));
                } else {
                    for entry in entries.iter().cloned() {
                        let item_owner = menu_owner.clone();
                        let item_editor = source_editor.clone();
                        let item_range = source_range.clone();
                        let item_selection = source_selection.clone();
                        let item_marker = marker.clone();
                        let mut label = entry.snippet.name.clone();
                        if entry.show_category {
                            label.push_str(&format!(" · {}", entry.snippet.category.label()));
                        }
                        if !entry.available {
                            let reason = entry
                                .unavailable_message
                                .as_deref()
                                .unwrap_or("Unavailable in this context");
                            label.push_str(&format!(" — {}", compact_label(reason, 42)));
                        }
                        let snippet_id = entry.snippet.id.clone();
                        submenu = submenu.item(
                            PopupMenuItem::new(label)
                                .disabled(!entry.available)
                                .on_click(move |_, window, cx| {
                                    if let Some(owner) = item_owner.upgrade() {
                                        owner.update(cx, |this, cx| {
                                            this.apply_snippet_from_menu(
                                                snippet_id.clone(),
                                                SnippetMenuInvocation {
                                                    source_editor: item_editor.clone(),
                                                    source_range: item_range.clone(),
                                                    selection: item_selection.clone(),
                                                    marker: item_marker.clone(),
                                                },
                                                window,
                                                cx,
                                            );
                                        });
                                    }
                                }),
                        );
                    }
                }
                let manage_owner = menu_owner.clone();
                submenu.separator().item(
                    PopupMenuItem::new("Manage snippets…")
                        .icon(IconName::CaseSensitive)
                        .on_click(move |_, window, cx| {
                            if let Some(owner) = manage_owner.upgrade() {
                                owner.update(cx, |this, cx| {
                                    this.open_workspace_tool_tab(
                                        WorkspaceToolTab::Snippets,
                                        window,
                                        cx,
                                    );
                                });
                            }
                        }),
                )
            })
        },
    )
}

impl ApiTester {
    pub(super) fn render_snippets_title_bar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dirty = self.snippet_editor_is_dirty(cx);
        h_flex()
            .h(px(APP_TITLE_BAR_HEIGHT))
            .flex_shrink_0()
            .pl(windows_controls::leading_inset())
            .pr_6()
            .border_b_1()
            .border_color(cx.theme().title_bar_border)
            .bg(cx.theme().title_bar)
            .justify_between()
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
                            .child(Icon::new(IconName::CaseSensitive).with_size(px(16.)))
                            .child("Snippets")
                            .when(dirty, |this| {
                                this.child(div().size(px(7.)).rounded_full().bg(cx.theme().warning))
                            }),
                    ),
            )
            .child(
                h_flex()
                    .h_full()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Right-click a code editor → Snippets"),
                    )
                    .child(windows_controls::windows_window_controls(window, cx))
            )
            .into_any_element()
    }

    pub(super) fn render_snippets_workspace(&self, cx: &mut Context<Self>) -> AnyElement {
        let query = self
            .snippet_editor
            .search
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        // Memoized list derivation: a notify caused by anything other than a
        // changed search query (or a snippet create/update/delete invalidating
        // the cache) reuses the last rows with zero re-derivation work.
        let rows = SNIPPET_LIST_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let is_hit = cache
                .memo
                .as_ref()
                .is_some_and(|(version, cached_query, _)| {
                    *version == cache.version && cached_query == &query
                });
            if is_hit {
                cache.memo.clone().expect("memo hit requires a memo").2
            } else {
                let rows = derive_snippet_list_rows(&self.snippets, &query, &mut cache.lower);
                cache.memo = Some((cache.version, query.clone(), rows.clone()));
                rows
            }
        });

        let list_rows = rows
            .iter()
            .map(|row| self.render_snippet_list_row(row, cx))
            .collect::<Vec<_>>();
        let this = cx.entity().downgrade();
        let new_this = this.clone();
        let save_this = this.clone();
        let duplicate_this = this.clone();
        let delete_this = this.clone();
        let preview_this = this.clone();
        let copy_this = this;
        let selected = self.snippet_editor.selected_id.is_some();
        let dirty = self.snippet_editor_is_dirty(cx);
        let preview_empty = !self.snippet_editor.preview_valid
            || self
                .snippet_editor
                .preview_editor
                .read(cx)
                .value(cx)
                .is_empty();
        let writable = true;
        let preview_help = snippet_preview_help(self.snippet_editor.category);

        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            .when_some(self.snippet_editor.notice.clone(), |this, notice| {
                this.child(super::settings_page::settings_message(
                    notice,
                    if self.snippet_editor.notice_is_error {
                        cx.theme().danger
                    } else {
                        cx.theme().info
                    },
                ))
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        v_flex()
                            .w(px(288.))
                            .h_full()
                            .flex_shrink_0()
                            .border_r_1()
                            .border_color(cx.theme().sidebar_border)
                            .bg(cx.api_surface_low())
                            .child(
                                h_flex()
                                    .h(px(56.))
                                    .px_4()
                                    .flex_shrink_0()
                                    .justify_between()
                                    .border_b_1()
                                    .border_color(cx.theme().sidebar_border)
                                    .child(div().text_base().font_semibold().child("Snippets"))
                                    .child(
                                        h_flex()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(format!(
                                                        "{} saved",
                                                        self.snippets.len()
                                                    )),
                                            )
                                            .child(
                                                Button::new("new-snippet")
                                                    .icon(IconName::Plus)
                                                    .small()
                                                    .ghost()
                                                    .rounded_full()
                                                    .tooltip("New snippet")
                                                    .disabled(!writable)
                                                    .on_click(move |_, window, cx| {
                                                        if let Some(this) = new_this.upgrade() {
                                                            this.update(cx, |this, cx| {
                                                                this.request_new_snippet(window, cx);
                                                            });
                                                        }
                                                    }),
                                            ),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .h(px(56.))
                                    .px_3()
                                    .flex_shrink_0()
                                    .border_b_1()
                                    .border_color(cx.theme().sidebar_border)
                                    .child(
                                        div().flex_1().min_w_0().child(
                                            Input::new(&self.snippet_editor.search)
                                                .prefix(IconName::Search)
                                                .cleanable(true),
                                        ),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .id("snippet-library-scroll")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .p_2()
                                    .gap_1()
                                    .children(list_rows)
                                    .when(rows.is_empty(), |this| {
                                        this.child(
                                            v_flex()
                                                .items_center()
                                                .gap_2()
                                                .px_5()
                                                .py_10()
                                                .text_center()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(
                                                    Icon::new(IconName::CaseSensitive)
                                                        .with_size(px(24.)),
                                                )
                                                .child(div().text_sm().child(if query.is_empty() {
                                                    "No snippets yet"
                                                } else {
                                                    "No matching snippets"
                                                }))
                                                .child(
                                                    div().text_xs().child(if query.is_empty() {
                                                        "Create one, then right-click in an editor and choose it from Snippets."
                                                    } else {
                                                        "Try a different name, description, or target."
                                                    }),
                                                ),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(
                                h_flex()
                                    .id("snippet-editor-toolbar")
                                    .w_full()
                                    .flex_shrink_0()
                                    .overflow_x_scroll()
                                    .gap_2()
                                    .px_4()
                                    .py_2()
                                    .border_b_1()
                                    .border_color(cx.api_outline_variant())
                                    .bg(cx.api_surface_low())
                                    .child(
                                        Button::new("save-snippet")
                                            .label(if selected { "Save changes" } else { "Create snippet" })
                                            .small()
                                            .primary()
                                            .disabled(!writable || (!dirty && selected))
                                            .on_click(move |_, _, cx| {
                                                if let Some(this) = save_this.upgrade() {
                                                    this.update(cx, |this, cx| this.save_snippet(cx));
                                                }
                                            }),
                                    )
                                    .child(
                                        Button::new("duplicate-snippet")
                                            .label("Duplicate")
                                            .small()
                                            .outline()
                                            .disabled(!writable)
                                            .on_click(move |_, window, cx| {
                                                if let Some(this) = duplicate_this.upgrade() {
                                                    this.update(cx, |this, cx| {
                                                        this.duplicate_snippet(window, cx);
                                                    });
                                                }
                                            }),
                                    )
                                    .child(
                                        Button::new("delete-snippet")
                                            .label("Delete…")
                                            .small()
                                            .ghost()
                                            .disabled(!writable || !selected)
                                            .on_click(move |_, window, cx| {
                                                if let Some(this) = delete_this.upgrade() {
                                                    this.update(cx, |this, cx| {
                                                        this.request_delete_snippet(window, cx);
                                                    });
                                                }
                                            }),
                                    )
                                    .child(div().flex_1())
                                    .when(dirty, |this| {
                                        this.child(
                                            div()
                                                .px_2()
                                                .py_1()
                                                .rounded_md()
                                                .bg(cx.theme().warning.opacity(0.12))
                                                .text_xs()
                                                .font_semibold()
                                                .text_color(cx.theme().warning)
                                                .child("Unsaved changes"),
                                        )
                                    }),
                            )
                            .child(
                                v_flex()
                                    .id("snippet-editor-scroll")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .p_5()
                                    .gap_5()
                                    .child(self.render_snippet_identity_section(cx))
                                    .child(self.render_snippet_target_section(cx))
                                    .child(self.render_snippet_behavior_section(cx))
                                    .child(self.render_snippet_code_section(cx))
                                    .child(
                                        v_flex()
                                            .gap_3()
                                            .pb_5()
                                            .child(snippet_step_header(
                                                "4",
                                                "Test the result",
                                                preview_help,
                                                cx,
                                            ))
                                            .child(
                                                h_flex()
                                                    .gap_2()
                                                    .child(
                                                        Button::new("run-snippet-preview")
                                                            .label(if self.snippet_editor.preview_running {
                                                                "Running…"
                                                            } else {
                                                                "Run preview"
                                                            })
                                                            .small()
                                                            .primary()
                                                            .disabled(self.snippet_editor.preview_running)
                                                            .on_click(move |_, window, cx| {
                                                                if let Some(this) = preview_this.upgrade() {
                                                                    this.update(cx, |this, cx| {
                                                                        this.run_snippet_preview(window, cx);
                                                                    });
                                                                }
                                                            }),
                                                    )
                                                    .child(
                                                        Button::new("copy-snippet-preview")
                                                            .label("Copy code")
                                                            .small()
                                                            .outline()
                                                            .disabled(preview_empty)
                                                            .on_click(move |_, _, cx| {
                                                                if let Some(this) = copy_this.upgrade() {
                                                                    this.update(cx, |this, cx| {
                                                                        this.copy_snippet_preview(cx);
                                                                    });
                                                                }
                                                            }),
                                                    )
                                                    .when_some(
                                                        self.snippet_editor.preview_status.clone(),
                                                        |this, status| {
                                                            this.child(
                                                                div()
                                                                    .min_w_0()
                                                                    .text_xs()
                                                                    .text_color(cx.theme().muted_foreground)
                                                                    .child(status),
                                                            )
                                                        },
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .w_full()
                                                    .min_h(px(190.))
                                                    .border_1()
                                                    .border_color(cx.api_outline_variant())
                                                    .rounded_lg()
                                                    .overflow_hidden()
                                                    .when(self.snippet_editor.preview_valid, |this| {
                                                        this.child(self.snippet_editor.preview_editor.clone())
                                                    })
                                                    .when(!self.snippet_editor.preview_valid, |this| {
                                                        this.child(
                                                            div()
                                                                .size_full()
                                                                .min_h(px(190.))
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .text_sm()
                                                                .text_color(cx.theme().muted_foreground)
                                                                .child(if self.snippet_editor.preview_running {
                                                                    "Generating preview…"
                                                                } else {
                                                                    "Run preview to see what will be inserted"
                                                                }),
                                                        )
                                                    }),
                                            )
                                            .when(!self.snippet_editor.preview_logs.is_empty(), |this| {
                                                this.child(self.render_snippet_preview_logs(cx))
                                            }),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_snippet_list_row(&self, row: &SnippetListRow, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.snippet_editor.selected_id.as_deref() == Some(&row.id);
        let id = row.id.clone();
        let description = (!row.description.is_empty()).then(|| compact_label(&row.description, 54));
        let leading_icon_color = if selected {
            cx.api_primary_bright()
        } else {
            cx.theme().muted_foreground
        };
        div()
            .id(SharedString::from(format!("snippet-row-{}", row.id)))
            .w_full()
            .flex()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.request_select_snippet(id.clone(), window, cx);
            }))
            .child(
                h_flex()
                    .w_full()
                    .flex_1()
                    .gap_2()
                    .px_2()
                    .rounded_md()
                    .when(selected, |this| this.bg(cx.theme().sidebar_accent))
                    .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.62)))
                    .child(
                        div().flex_shrink_0().flex().items_center().child(
                            Icon::new(IconName::CaseSensitive)
                                .small()
                                .text_color(leading_icon_color),
                        ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_sm()
                                    .font_semibold()
                                    .child(row.name.clone()),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .whitespace_nowrap()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(row.category.label()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("·"),
                                    )
                                    .child(
                                        div()
                                            .whitespace_nowrap()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(row.kind.label()),
                                    ),
                            )
                            .when_some(description, |this, description| {
                                this.child(
                                    div()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(description),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_snippet_identity_section(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap_3()
            .child(snippet_step_header(
                "1",
                "Name the snippet",
                "Names appear in the Snippets menu wherever the snippet can be used.",
                cx,
            ))
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(snippet_field_label("Name", true, cx))
                            .child(
                                Input::new(&self.snippet_editor.name)
                                    .disabled(false),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(snippet_field_label("Description", false, cx))
                            .child(
                                Input::new(&self.snippet_editor.description)
                                    .disabled(false),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_snippet_target_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        let pre_this = this.clone();
        let post_this = this;
        v_flex()
            .gap_3()
            .child(snippet_step_header(
                "2",
                "Choose the target",
                "Pre-request adds code to the script that runs before sending. Post-response adds code to the script that runs after a response arrives.",
                cx,
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("snippet-category-pre-request")
                            .label("Pre-request")
                            .outline()
                            .disabled(false)
                            .selected(self.snippet_editor.category == SnippetCategory::PreRequest)
                            .on_click(move |_, window, cx| {
                                if let Some(this) = pre_this.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.set_snippet_category(
                                            SnippetCategory::PreRequest,
                                            window,
                                            cx,
                                        );
                                    });
                                }
                            }),
                    )
                    .child(
                        Button::new("snippet-category-post-response")
                            .label("Post-response")
                            .outline()
                            .disabled(false)
                            .selected(
                                self.snippet_editor.category == SnippetCategory::PostResponse,
                            )
                            .on_click(move |_, window, cx| {
                                if let Some(this) = post_this.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.set_snippet_category(
                                            SnippetCategory::PostResponse,
                                            window,
                                            cx,
                                        );
                                    });
                                }
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_snippet_behavior_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        let plain_this = this.clone();
        let executable_this = this;
        let requirements = [
            (
                SnippetRequirement::HasResponse,
                "Current response",
                "Show only after a response arrives.",
            ),
            (
                SnippetRequirement::HasSelection,
                "Selected block",
                "Show only when text is selected.",
            ),
            (
                SnippetRequirement::HasRequestSelection,
                "Request selection",
                "Show only when text is selected in the request body or pre-request script.",
            ),
            (
                SnippetRequirement::HasResponseSelection,
                "Response selection",
                "Show only when text is selected in the response body or post-response script.",
            ),
            (
                SnippetRequirement::HasJsonSelection,
                "Selected JSON value",
                "Show only when a value or field is selected in a JSON body.",
            ),
        ];
        let requirement_controls = requirements
            .into_iter()
            .enumerate()
            .map(|(index, (requirement, label, detail))| {
                let checked = self.snippet_editor.requirements.contains(&requirement);
                let unavailable_for_phase = requirement == SnippetRequirement::HasResponse
                    && self.snippet_editor.category == SnippetCategory::PreRequest;
                let disabled = unavailable_for_phase;
                v_flex()
                    .gap_0p5()
                    .child(
                        Checkbox::new(("snippet-requirement", index))
                            .label(label)
                            .checked(checked)
                            .disabled(disabled)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                this.toggle_snippet_requirement(requirement, *checked, cx);
                            })),
                    )
                    .child(
                        div()
                            .pl_6()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(if unavailable_for_phase {
                                "Responses are available only to post-response snippets."
                            } else {
                                detail
                            }),
                    )
            })
            .collect::<Vec<_>>();

        v_flex()
            .gap_3()
            .child(snippet_step_header(
                "3",
                "Define the behavior",
                "Insert the code as written, or use JavaScript to build it when you choose the snippet.",
                cx,
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("snippet-kind-plain")
                            .label("Plain JavaScript")
                            .outline()
                            .disabled(false)
                            .selected(self.snippet_editor.kind == SnippetKind::Plain)
                            .on_click(move |_, window, cx| {
                                if let Some(this) = plain_this.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.set_snippet_kind(SnippetKind::Plain, window, cx);
                                    });
                                }
                            }),
                    )
                    .child(
                        Button::new("snippet-kind-executable")
                            .label("Executable generator")
                            .outline()
                            .disabled(false)
                            .selected(self.snippet_editor.kind == SnippetKind::Executable)
                            .on_click(move |_, window, cx| {
                                if let Some(this) = executable_this.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.set_snippet_kind(
                                            SnippetKind::Executable,
                                            window,
                                            cx,
                                        );
                                    });
                                }
                            }),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.api_outline_variant())
                    .child(
                        h_flex()
                            .justify_between()
                            .child(div().text_sm().font_semibold().child("Show in menu when"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if self.snippet_editor.requirements.is_empty() {
                                        "No extra conditions"
                                    } else {
                                        "All selected conditions"
                                    }),
                            ),
                    )
                    .children(requirement_controls),
            )
            .into_any_element()
    }

    fn render_snippet_code_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let heading = match self.snippet_editor.kind {
            SnippetKind::Plain => "Snippet JavaScript",
            SnippetKind::Executable => "Generator JavaScript",
        };
        let detail = snippet_source_help(self.snippet_editor.category, self.snippet_editor.kind);
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .justify_between()
                    .child(div().text_sm().font_semibold().child(heading))
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child("JavaScript"),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(detail),
            )
            .child(
                div()
                    .w_full()
                    .min_h(px(360.))
                    .border_1()
                    .border_color(cx.api_outline_variant())
                    .rounded_lg()
                    .overflow_hidden()
                    .child(self.snippet_editor.editor.clone()),
            )
            .into_any_element()
    }

    fn render_snippet_preview_logs(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows = self
            .snippet_editor
            .preview_logs
            .iter()
            .enumerate()
            .map(|(index, log)| {
                h_flex()
                    .id(("snippet-preview-log", index))
                    .gap_2()
                    .text_xs()
                    .child(
                        div()
                            .w(px(44.))
                            .flex_shrink_0()
                            .font_semibold()
                            .text_color(match log.level {
                                crate::core::SnippetLogLevel::Warn => cx.theme().warning,
                                crate::core::SnippetLogLevel::Error => cx.theme().danger,
                                _ => cx.theme().muted_foreground,
                            })
                            .child(format!("{:?}", log.level).to_ascii_lowercase()),
                    )
                    .child(div().min_w_0().child(log.message.clone()))
            })
            .collect::<Vec<_>>();
        v_flex()
            .gap_1()
            .p_3()
            .rounded_lg()
            .bg(cx.api_surface_low())
            .border_1()
            .border_color(cx.api_outline_variant())
            .child(div().text_xs().font_semibold().child("Generator console"))
            .children(rows)
            .into_any_element()
    }
}

fn format_snippet_source(
    source: &str,
    settings: &crate::core::FormatterSettings,
) -> Result<String, String> {
    crate::core::format_script_source(source, settings)
}

/// Lowercase search fields for one snippet, computed once per snippet-set
/// version and reused across query changes so re-typing in the search box never
/// repeats the per-field `to_lowercase` heap allocations.
struct SnippetSearchFields {
    name_lower: String,
    description_lower: String,
    category_label_lower: String,
}

impl SnippetSearchFields {
    fn from_snippet(snippet: &Snippet) -> Self {
        Self {
            name_lower: snippet.name.to_lowercase(),
            description_lower: snippet.description.to_lowercase(),
            category_label_lower: snippet.category.label().to_lowercase(),
        }
    }
}

/// Lightweight library row for one snippet. Carries only the fields the list
/// row renders (plus lowercase search fields); the potentially multi-KB
/// `Snippet::source` body is never copied onto the list path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SnippetListRow {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) description: String,
    pub(super) category: SnippetCategory,
    pub(super) kind: SnippetKind,
    pub(super) name_lower: String,
    pub(super) description_lower: String,
    pub(super) category_label_lower: String,
}

impl SnippetListRow {
    fn from_parts(
        id: String,
        name: String,
        description: String,
        category: SnippetCategory,
        kind: SnippetKind,
        name_lower: String,
        description_lower: String,
        category_label_lower: String,
    ) -> Self {
        Self {
            id,
            name,
            description,
            category,
            kind,
            name_lower,
            description_lower,
            category_label_lower,
        }
    }
}

/// Render cache for the snippets library list, sized to the single running app
/// instance (GPUI drives one main-thread event loop). Every snippet
/// create/update/delete site bumps `version`, which invalidates both the cached
/// lowercase search fields and the memoized rows below.
#[derive(Default)]
struct SnippetListCache {
    version: u64,
    lower: HashMap<String, SnippetSearchFields>,
    memo: Option<(u64, String, Vec<SnippetListRow>)>,
}

thread_local! {
    static SNIPPET_LIST_CACHE: RefCell<SnippetListCache> = RefCell::new(SnippetListCache::default());
}

impl SnippetListCache {
    fn invalidate(&mut self) {
        self.version = self.version.wrapping_add(1);
        self.lower.clear();
        self.memo = None;
    }
}

/// Derive the filtered, category-ordered library rows for `query` on top of
/// cached per-snippet lowercase fields. This is the only place the list path
/// walks `Snippet`s; it never clones a `Snippet`, so source bodies are not
/// copied per row.
fn derive_snippet_list_rows(
    snippets: &[Snippet],
    query: &str,
    lower: &mut HashMap<String, SnippetSearchFields>,
) -> Vec<SnippetListRow> {
    lower.retain(|id, _| snippets.iter().any(|snippet| &snippet.id == id));
    let rows = snippets
        .iter()
        .map(|snippet| {
            let fields = lower
                .entry(snippet.id.clone())
                .or_insert_with(|| SnippetSearchFields::from_snippet(snippet));
            SnippetListRow::from_parts(
                snippet.id.clone(),
                snippet.name.clone(),
                snippet.description.clone(),
                snippet.category,
                snippet.kind,
                fields.name_lower.clone(),
                fields.description_lower.clone(),
                fields.category_label_lower.clone(),
            )
        })
        .collect::<Vec<_>>();
    filter_snippet_list_rows(query, rows)
}

/// Pure filter + sort over already-derived rows. An empty query keeps every
/// snippet; otherwise a `contains` over the pre-lowercased name, description,
/// and category-label fields. `query` is expected to be lowercased already
/// (the render path lowercases the search input), matching the original
/// behavior. Rows are ordered by category (Pre-request before Post-response),
/// then by lowercased name.
pub(super) fn filter_snippet_list_rows(
    query: &str,
    rows: Vec<SnippetListRow>,
) -> Vec<SnippetListRow> {
    let mut matched = if query.is_empty() {
        rows
    } else {
        rows.into_iter()
            .filter(|row| {
                row.name_lower.contains(query)
                    || row.description_lower.contains(query)
                    || row.category_label_lower.contains(query)
            })
            .collect::<Vec<_>>()
    };
    matched.sort_by(|left, right| {
        snippet_category_order(left.category)
            .cmp(&snippet_category_order(right.category))
            .then_with(|| left.name_lower.cmp(&right.name_lower))
    });
    matched
}

fn snippet_category_order(category: SnippetCategory) -> u8 {
    match category {
        SnippetCategory::PreRequest => 0,
        SnippetCategory::PostResponse => 1,
    }
}

fn snippet_source_help(category: SnippetCategory, kind: SnippetKind) -> &'static str {
    match (category, kind) {
        (SnippetCategory::PreRequest, SnippetKind::Plain) => {
            "This JavaScript is added to the pre-request script exactly as written."
        }
        (SnippetCategory::PostResponse, SnippetKind::Plain) => {
            "This JavaScript is added to the post-response script exactly as written."
        }
        (SnippetCategory::PreRequest, SnippetKind::Executable) => {
            "Use the current request and selected text, if any, to build the JavaScript that will be added."
        }
        (SnippetCategory::PostResponse, SnippetKind::Executable) => {
            "Use the current request, response, and selected text, if any, to build the JavaScript that will be added."
        }
    }
}

fn snippet_preview_help(category: SnippetCategory) -> &'static str {
    match category {
        SnippetCategory::PreRequest => {
            "See the code this snippet will insert using the current request and selected text, if any."
        }
        SnippetCategory::PostResponse => {
            "See the code this snippet will insert using the current request, response, and selected text, if any."
        }
    }
}

fn snippet_step_header(
    number: &'static str,
    title: &'static str,
    description: &'static str,
    cx: &mut Context<ApiTester>,
) -> AnyElement {
    h_flex()
        .items_start()
        .gap_3()
        .child(
            h_flex()
                .size(px(26.))
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(cx.theme().primary.opacity(0.14))
                .text_xs()
                .font_bold()
                .text_color(cx.theme().primary)
                .child(number),
        )
        .child(
            v_flex()
                .gap_0p5()
                .child(div().text_sm().font_semibold().child(title))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(description),
                ),
        )
        .into_any_element()
}

fn snippet_field_label(
    label: &'static str,
    required: bool,
    cx: &mut Context<ApiTester>,
) -> AnyElement {
    h_flex()
        .gap_1()
        .text_xs()
        .font_semibold()
        .child(label)
        .when(required, |this| {
            this.child(div().text_color(cx.theme().danger).child("*"))
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snippet(name: &str, category: SnippetCategory) -> Snippet {
        Snippet::new(name, category, SnippetKind::Plain).unwrap()
    }

    fn draft(name: &str, description: &str) -> SnippetDraftSnapshot {
        SnippetDraftSnapshot {
            name: name.to_owned(),
            description: description.to_owned(),
            ..SnippetDraftSnapshot::empty()
        }
    }

    fn test_response(body: &'static [u8]) -> ResponseData {
        ResponseData {
            status: 200,
            status_text: "OK".to_owned(),
            http_version: "HTTP/1.1".to_owned(),
            final_url: "https://example.test/".to_owned(),
            headers: Vec::new(),
            content_type: Some("application/json".to_owned()),
            body: bytes::Bytes::from_static(body),
            duration: Duration::from_millis(25),
        }
    }

    #[test]
    fn snippet_menu_marker_rejects_every_context_race() {
        let tab_id = RequestTabId::new();
        let response = test_response(b"old");
        let marker = SnippetMenuInvocationMarker {
            source_document: "selected source".into(),
            request_tab_id: tab_id.clone(),
            request_generation: 7,
            response_identity: response_identity(Some(&response)),
        };

        assert_eq!(
            marker.stale_reason("selected source", &tab_id, 7, Some(&response)),
            None
        );
        assert_eq!(
            marker.stale_reason("changed source", &tab_id, 7, Some(&response)),
            Some(SnippetMenuStaleReason::SourceDocument)
        );
        assert_eq!(
            marker.stale_reason("selected source", &RequestTabId::new(), 7, Some(&response),),
            Some(SnippetMenuStaleReason::ActiveRequestTab)
        );
        assert_eq!(
            marker.stale_reason("selected source", &tab_id, 8, Some(&response)),
            Some(SnippetMenuStaleReason::RequestGeneration)
        );

        let changed_response = test_response(b"new");
        assert_eq!(
            marker.stale_reason("selected source", &tab_id, 7, Some(&changed_response)),
            Some(SnippetMenuStaleReason::ResponseState)
        );
        let mut reallocated_response = response.clone();
        reallocated_response.body = bytes::Bytes::copy_from_slice(&response.body);
        assert_eq!(
            marker.stale_reason("selected source", &tab_id, 7, Some(&reallocated_response)),
            Some(SnippetMenuStaleReason::ResponseState)
        );
        assert_eq!(
            marker.stale_reason("selected source", &tab_id, 7, None),
            Some(SnippetMenuStaleReason::ResponseState)
        );
    }

    #[test]
    fn generated_script_size_limit_accepts_the_exact_boundary() {
        let source = "x".repeat(crate::core::MAX_SCRIPT_SOURCE_BYTES - 1);
        let end = source.encode_utf16().count();

        assert_eq!(
            script_source_bytes_after_replacement(&source, end..end, "y"),
            Some(crate::core::MAX_SCRIPT_SOURCE_BYTES)
        );
        assert!(
            script_source_bytes_after_replacement(&source, end..end, "yz").unwrap()
                > crate::core::MAX_SCRIPT_SOURCE_BYTES
        );
        assert_eq!(
            script_source_bytes_after_replacement("a😀z", 1..3, "é"),
            Some(4)
        );
    }

    #[test]
    fn stale_response_editor_text_is_not_a_current_selection() {
        let selection = SnippetSelection::new(
            SnippetSelectionSource::Response,
            SnippetSelectionArea::Body,
            "stale",
            SnippetTextRange::new(0, 5),
            Some("text/plain".to_owned()),
        )
        .unwrap();

        assert!(
            current_surface_selection(
                SnippetMenuSurface::ResponseBody,
                false,
                Some(selection.clone()),
            )
            .is_none()
        );
        assert_eq!(
            current_surface_selection(
                SnippetMenuSurface::ResponseBody,
                true,
                Some(selection.clone())
            ),
            Some(selection.clone())
        );
        assert_eq!(
            current_surface_selection(
                SnippetMenuSurface::RequestBody,
                false,
                Some(selection.clone())
            ),
            Some(selection)
        );
    }

    #[test]
    fn canonical_snippet_draft_values_remain_clean() {
        let baseline = draft("Logger", "Writes a log statement");

        assert!(!snippet_draft_is_dirty(&baseline, &baseline));
    }

    #[test]
    fn saved_text_normalization_does_not_leave_the_draft_dirty() {
        let baseline = draft("Logger", "Writes a log statement");
        let padded = draft("  Logger\n", "\tWrites a log statement  ");

        assert!(!snippet_draft_is_dirty(&padded, &baseline));

        let changed = draft("  Different name  ", "\tWrites a log statement  ");
        assert!(snippet_draft_is_dirty(&changed, &baseline));
    }

    #[test]
    fn changed_snippet_drafts_are_dirty() {
        let baseline = draft("Logger", "Writes a log statement");
        let mut changed = draft("Different name", "Different description");
        changed.source = "console.log('changed');".to_owned();

        assert!(snippet_draft_is_dirty(&changed, &baseline));
    }

    #[test]
    fn duplicate_names_are_scoped_by_category_and_case_insensitive() {
        let snippets = vec![
            snippet("Logger copy", SnippetCategory::PreRequest),
            snippet("LOGGER COPY 2", SnippetCategory::PreRequest),
            snippet("Logger copy 3", SnippetCategory::PostResponse),
        ];
        assert_eq!(
            unique_snippet_name("Logger", SnippetCategory::PreRequest, &snippets),
            "Logger copy 3"
        );
        assert_eq!(
            unique_snippet_name("Logger", SnippetCategory::PostResponse, &snippets),
            "Logger copy"
        );
    }

    #[test]
    fn duplicate_name_generation_is_unicode_aware_and_utf8_bounded() {
        let full_name = "é".repeat(MAX_SNIPPET_NAME_BYTES / 2);
        let snippets = vec![
            snippet(&full_name, SnippetCategory::PreRequest),
            snippet("Ä copy", SnippetCategory::PostResponse),
        ];

        let bounded = unique_snippet_name(&full_name, SnippetCategory::PreRequest, &snippets);
        assert!(bounded.len() <= MAX_SNIPPET_NAME_BYTES);
        assert_ne!(bounded.to_lowercase(), full_name.to_lowercase());
        Snippet::new(bounded, SnippetCategory::PreRequest, SnippetKind::Plain).unwrap();

        assert_eq!(
            unique_snippet_name("ä", SnippetCategory::PostResponse, &snippets),
            "ä copy 2"
        );
    }

    #[test]
    fn script_surfaces_filter_to_their_target_but_body_surfaces_show_both() {
        assert_eq!(
            SnippetMenuSurface::PreRequestScript.target_category(),
            Some(SnippetCategory::PreRequest)
        );
        assert_eq!(
            SnippetMenuSurface::PostResponseScript.target_category(),
            Some(SnippetCategory::PostResponse)
        );
        assert_eq!(SnippetMenuSurface::RequestBody.target_category(), None);
        assert_eq!(SnippetMenuSurface::ResponseBody.target_category(), None);
    }

    #[test]
    fn snippet_source_help_explains_the_available_context_in_plain_language() {
        let pre = snippet_source_help(SnippetCategory::PreRequest, SnippetKind::Executable);
        let post = snippet_source_help(SnippetCategory::PostResponse, SnippetKind::Executable);

        assert_eq!(
            pre,
            "Use the current request and selected text, if any, to build the JavaScript that will be added."
        );
        assert_eq!(
            post,
            "Use the current request, response, and selected text, if any, to build the JavaScript that will be added."
        );
        assert_eq!(
            snippet_source_help(SnippetCategory::PreRequest, SnippetKind::Plain),
            "This JavaScript is added to the pre-request script exactly as written."
        );
        assert_eq!(
            snippet_source_help(SnippetCategory::PostResponse, SnippetKind::Plain),
            "This JavaScript is added to the post-response script exactly as written."
        );
        assert_eq!(
            snippet_preview_help(SnippetCategory::PreRequest),
            "See the code this snippet will insert using the current request and selected text, if any."
        );
        assert_eq!(
            snippet_preview_help(SnippetCategory::PostResponse),
            "See the code this snippet will insert using the current request, response, and selected text, if any."
        );
    }

    #[test]
    fn executable_function_body_formats_without_changing_template_text() {
        let source = "return `first line\n  literal indent ${api.request.method}`";
        let formatted =
            format_snippet_source(source, &crate::core::FormatterSettings::default()).unwrap();

        assert!(formatted.starts_with("return `first line\n  literal indent"));
        assert!(formatted.contains("${api.request.method}`;"));
    }
}
