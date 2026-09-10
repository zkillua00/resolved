use super::drag_drop::WorkspaceTabDrag;
use super::*;

/// One full request-editor session attached to a secondary workspace pane.
///
/// Every pane that does not host the primary request surface gets its own
/// [`PaneEditorState`], so a tab moved into that pane renders a real request
/// editor (method/URL/body/headers/scripts) driven by editor entities owned by
/// the session rather than a placeholder.
///
/// The request *records* still live globally in `self.request_tabs`; this
/// struct only holds the presentation state (what sits in the editors right
/// now) plus the pane's own copies of the runtime response fields.
pub(in crate::app) struct PaneEditorState {
    active_tab_id: Option<RequestTabId>,
    method: Entity<InputState>,
    url: Entity<InputState>,
    body: Entity<CodeEditor>,
    documentation: Entity<CodeEditor>,
    documentation_intelligence: Rc<crate::documentation_intelligence::DocumentationIntelligence>,
    pre_request_script: Entity<CodeEditor>,
    post_response_script: Entity<CodeEditor>,
    response_editor: Entity<CodeEditor>,
    query_params: Vec<QueryParamRow>,
    next_query_param_id: usize,
    headers: Vec<HeaderRow>,
    next_header_id: usize,
    body_mode: BodyMode,
    raw_body_language: RawBodyLanguage,
    body_fields: Vec<request_body_editor::BodyFieldRow>,
    next_body_field_id: usize,
    request_pane: RequestPane,
    response_tab: ResponseTab,
    pretty_body: bool,
    response: Option<ResponseData>,
    response_request: Option<RequestDraft>,
    response_sensitive_values: Vec<String>,
    /// Pre-formatted response body for the Body/Preview tabs. Rebuilt only when
    /// the response or the pretty toggle changes, never per frame (the primary
    /// surface caches this in its response editor; rendering the raw body every
    /// repaint re-parsed/re-copied up to 64 MiB on each frame).
    formatted_body: Option<SharedString>,
    request_error: Option<String>,
    script_diagnostic: Option<ScriptDiagnostic>,
    pre_script_report: Option<ScriptReport>,
    post_script_report: Option<ScriptReport>,
    script_console_reports: Vec<ScriptReport>,
    script_console_hidden_rows: usize,
    preview_error: Option<String>,
    copied: bool,
    request_notice: Option<String>,
    pub(in crate::app) pane_scroll: ScrollHandle,
    _url_subscription: Subscription,
    _method_subscription: Subscription,
}

impl PaneEditorState {
    fn new(
        pane_id: PaneId,
        active_tab_id: RequestTabId,
        window: &mut Window,
        cx: &mut Context<ApiTester>,
    ) -> Self {
        let snippet_menu_owner = cx.entity().downgrade();
        let method = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("METHOD")
                .default_value("GET")
        });
        let method_subscription = cx.subscribe(&method, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("URL"));
        let url_subscription = cx.subscribe_in(&url, window, move |this, _, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                this.pane_sync_query_params_from_url(pane_id, window, cx);
            }
        });
        let (documentation, documentation_intelligence) = documentation::new_editor(window, cx);
        let body_hover =
            documentation::body_hover_provider(documentation_intelligence.clone(), &documentation);
        let body = cx.new(|cx| {
            CodeEditor::new(
                CodeEditorConfig::default()
                    .framed(false)
                    .embedded(true)
                    .language(CodeLanguage::Json)
                    .placeholder("Raw request body")
                    .rows(12)
                    .soft_wrap(false)
                    .format_action(true)
                    .context_menu_builder(snippet_context_menu_builder(
                        snippet_menu_owner.clone(),
                        SnippetMenuSurface::RequestBody,
                    ))
                    .hover_provider(body_hover),
                window,
                cx,
            )
        });
        // Body intelligence is refreshed at the request-panel boundary, so a
        // secondary editor change must invalidate that parent surface as well.
        cx.subscribe(&body, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        let pre_request_script = cx.new(|cx| {
            CodeEditor::new(
                CodeEditorConfig::default()
                    .framed(false)
                    .embedded(true)
                    .language(CodeLanguage::JavaScript)
                    .placeholder("api.request.headers.set(\"X-Token\", \"value\");")
                    .rows(12)
                    .soft_wrap(false)
                    .format_action(true)
                    .context_menu_builder(snippet_context_menu_builder(
                        snippet_menu_owner.clone(),
                        SnippetMenuSurface::PreRequestScript,
                    )),
                window,
                cx,
            )
        });
        let post_response_script = cx.new(|cx| {
            CodeEditor::new(
                CodeEditorConfig::default()
                    .framed(false)
                    .embedded(true)
                    .language(CodeLanguage::JavaScript)
                    .placeholder("api.test(\"status is 200\", () => api.assert(api.response.status === 200));")
                    .rows(12)
                    .soft_wrap(false)
                    .format_action(true)
                    .context_menu_builder(snippet_context_menu_builder(
                        snippet_menu_owner,
                        SnippetMenuSurface::PostResponseScript,
                    )),
                window,
                cx,
            )
        });
        let response_editor = cx.new(|cx| {
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(CodeLanguage::Json)
                    .placeholder("Response body")
                    .framed(false)
                    .rows(20)
                    .soft_wrap(false)
                    .read_only(true),
                window,
                cx,
            )
        });
        let mut this = Self {
            active_tab_id: Some(active_tab_id),
            method,
            url,
            body,
            documentation,
            documentation_intelligence,
            pre_request_script,
            post_response_script,
            response_editor,
            query_params: Vec::new(),
            next_query_param_id: 0,
            headers: Vec::new(),
            next_header_id: 0,
            body_mode: BodyMode::Raw,
            raw_body_language: RawBodyLanguage::Json,
            body_fields: Vec::new(),
            next_body_field_id: 0,
            request_pane: RequestPane::Params,
            response_tab: ResponseTab::Body,
            pretty_body: true,
            response: None,
            response_request: None,
            response_sensitive_values: Vec::new(),
            formatted_body: None,
            request_error: None,
            script_diagnostic: None,
            pre_script_report: None,
            post_script_report: None,
            script_console_reports: Vec::new(),
            script_console_hidden_rows: 0,
            preview_error: None,
            copied: false,
            request_notice: None,
            pane_scroll: ScrollHandle::default(),
            _url_subscription: url_subscription,
            _method_subscription: method_subscription,
        };
        this.set_raw_body_language(cx);
        this
    }

    fn set_raw_body_language(&mut self, cx: &mut Context<ApiTester>) {
        let language = code_language_for_raw_body(self.raw_body_language);
        self.body
            .update(cx, |editor, cx| editor.set_language(language, cx));
    }

    /// Refresh the raw body editor's highlight to match `raw_body_language`.
    fn refresh_raw_body_language(&mut self, cx: &mut Context<ApiTester>) {
        let language = code_language_for_raw_body(self.raw_body_language);
        self.body
            .update(cx, |editor, cx| editor.set_language(language, cx));
    }

    #[allow(clippy::too_many_arguments)]
    fn push_query_param_row(
        &mut self,
        pane_id: PaneId,
        key: impl Into<SharedString>,
        value: impl Into<SharedString>,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<ApiTester>,
    ) {
        let id = self.next_query_param_id;
        self.next_query_param_id = self.next_query_param_id.wrapping_add(1);
        let key_state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Key")
                .default_value(key.into())
        });
        let value_state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Value")
                .default_value(value.into())
        });
        let key_subscription =
            cx.subscribe_in(&key_state, window, move |this, _, event, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.pane_sync_url_from_query_params(pane_id, window, cx);
                }
            });
        let value_subscription =
            cx.subscribe_in(&value_state, window, move |this, _, event, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.pane_sync_url_from_query_params(pane_id, window, cx);
                }
            });
        self.query_params.push(QueryParamRow {
            id,
            key: key_state,
            value: value_state,
            enabled,
            _subscriptions: vec![key_subscription, value_subscription],
        });
    }

    fn push_header_row(
        &mut self,
        name: impl Into<SharedString>,
        value: impl Into<SharedString>,
        enabled: bool,
        shared: bool,
        window: &mut Window,
        cx: &mut Context<ApiTester>,
    ) {
        let id = self.next_header_id;
        self.next_header_id = self.next_header_id.wrapping_add(1);
        let name_state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Key")
                .default_value(name.into())
        });
        let value_state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Value")
                .default_value(value.into())
        });
        self.headers.push(HeaderRow {
            id,
            name: name_state,
            value: value_state,
            enabled,
            shared,
            _subscriptions: Vec::new(),
        });
    }

    fn push_body_field_row(
        &mut self,
        name: impl Into<SharedString>,
        value: impl Into<SharedString>,
        enabled: bool,
        kind: BodyFieldKind,
        window: &mut Window,
        cx: &mut Context<ApiTester>,
    ) {
        let id = self.next_body_field_id;
        self.next_body_field_id = self.next_body_field_id.wrapping_add(1);
        let name_state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Field")
                .default_value(name.into())
        });
        let value_state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Value")
                .default_value(value.into())
        });
        self.body_fields.push(request_body_editor::BodyFieldRow {
            id,
            name: name_state,
            value: value_state,
            enabled,
            kind,
            _subscriptions: Vec::new(),
        });
    }

    /// Load a request template plus its persisted runtime into this session.
    fn load_template(
        &mut self,
        pane_id: PaneId,
        template: &RequestTemplate,
        runtime: &RequestTabRuntime,
        formatter: &FormatterSettings,
        window: &mut Window,
        cx: &mut Context<ApiTester>,
    ) {
        self.documentation.update(cx, |editor, cx| {
            editor.set_value(template.documentation.clone(), window, cx)
        });
        self.body_mode = template.request.body_mode;
        self.raw_body_language = template.request.raw_body_language;
        self.method.update(cx, |state, cx| {
            state.set_value(template.request.method.clone(), window, cx);
        });
        self.url.update(cx, |state, cx| {
            state.set_value(template.request.url.clone(), window, cx);
        });
        self.body.update(cx, |state, cx| {
            state.set_value(template.request.body.clone(), window, cx);
        });
        self.pre_request_script.update(cx, |editor, cx| {
            editor.set_value(template.scripts.pre_request.clone(), window, cx);
        });
        self.post_response_script.update(cx, |editor, cx| {
            editor.set_value(template.scripts.post_response.clone(), window, cx);
        });

        self.query_params.clear();
        let query_params = if template.request.query_params.is_empty() {
            query_params_from_url(&template.request.url)
        } else {
            template.request.query_params.clone()
        };
        for param in query_params {
            self.push_query_param_row(
                pane_id,
                param.key,
                param.value,
                param.enabled,
                window,
                cx,
            );
        }
        if self.query_params.is_empty() {
            self.push_query_param_row(pane_id, "", "", true, window, cx);
        }

        self.headers.clear();
        for header in &template.request.headers {
            let was_redacted = header.value == REDACTED_VALUE;
            self.push_header_row(
                header.name.clone(),
                if was_redacted {
                    String::new()
                } else {
                    header.value.clone()
                },
                header.enabled && !was_redacted,
                header.shared,
                window,
                cx,
            );
        }
        if self.headers.is_empty() {
            self.push_header_row("", "", true, true, window, cx);
        }

        self.body_fields.clear();
        for field in &template.request.body_fields {
            self.push_body_field_row(
                field.name.clone(),
                field.value.clone(),
                field.enabled,
                field.kind,
                window,
                cx,
            );
        }
        if matches!(
            self.body_mode,
            BodyMode::FormUrlEncoded | BodyMode::MultipartFormData
        ) && self.body_fields.is_empty()
        {
            self.push_body_field_row("", "", true, BodyFieldKind::Text, window, cx);
        }
        self.refresh_raw_body_language(cx);

        self.request_pane = runtime.request_pane;
        self.response_tab = runtime.response_tab;
        self.pretty_body = runtime.pretty_body;
        self.response = runtime.response.clone();
        self.formatted_body = self.response.as_ref().and_then(|response| {
            is_probably_text(&response.body).then(|| {
                SharedString::from(format_body(&response.body, self.pretty_body, formatter))
            })
        });
        self.response_request = runtime.response_request.clone();
        self.response_sensitive_values = runtime.response_sensitive_values.clone();
        self.request_error = runtime.request_error.clone();
        self.script_diagnostic = runtime.script_diagnostic.clone();
        self.pre_script_report = runtime.pre_script_report.clone();
        self.post_script_report = runtime.post_script_report.clone();
        self.script_console_reports = runtime.script_console_reports.clone();
        self.script_console_hidden_rows = runtime.script_console_hidden_rows;
        self.preview_error = runtime.preview_error.clone();
        self.copied = runtime.copied;
        self.request_notice = runtime.request_notice.clone();
        if let Some(response) = &self.response {
            let language = response_language(response);
            let content = self.formatted_body.clone().map_or_else(
                || format!("Binary response ({}).", format_bytes(response.size_bytes())),
                |body| body.to_string(),
            );
            self.response_editor.update(cx, |editor, cx| {
                editor.set_language(language, cx);
                editor.set_value(content, window, cx);
            });
        } else {
            self.response_editor.update(cx, |editor, cx| {
                editor.set_value(String::new(), window, cx);
            });
        }
    }

    /// Snapshot the current editor contents back into a request template.
    fn snapshot_template(&self, cx: &App) -> RequestTemplate {
        let query_params = self.normalized_query_params(cx);
        let url = url_with_query_params(self.url.read(cx).value().as_ref(), &query_params);
        RequestTemplate {
            request: RequestDraft {
                method: self.method.read(cx).value().to_string(),
                url,
                query_params,
                headers: self
                    .headers
                    .iter()
                    .map(|row| HeaderEntry {
                        enabled: row.enabled,
                        shared: row.shared,
                        name: row.name.read(cx).value().to_string(),
                        value: row.value.read(cx).value().to_string(),
                    })
                    .collect(),
                body: self.body.read(cx).value(cx).to_string(),
                body_mode: self.body_mode,
                raw_body_language: self.raw_body_language,
                body_fields: self
                    .body_fields
                    .iter()
                    .map(|row| BodyField {
                        enabled: row.enabled,
                        name: row.name.read(cx).value().to_string(),
                        value: row.value.read(cx).value().to_string(),
                        kind: row.kind,
                    })
                    .collect(),
            },
            scripts: RequestScripts {
                pre_request: self.pre_request_script.read(cx).value(cx).to_string(),
                post_response: self.post_response_script.read(cx).value(cx).to_string(),
            },
            documentation: self.documentation.read(cx).value(cx).to_string(),
            websocket: None,
        }
    }

    fn normalized_query_params(&self, cx: &App) -> Vec<QueryParamEntry> {
        self.query_params
            .iter()
            .filter_map(|row| {
                let key = row.key.read(cx).value().to_string();
                let value = row.value.read(cx).value().to_string();
                if key.trim().is_empty() && value.trim().is_empty() {
                    return None;
                }
                Some(QueryParamEntry {
                    enabled: row.enabled,
                    key,
                    value,
                })
            })
            .collect()
    }

    /// Build the per-pane runtime snapshot keyed by a request tab id.
    fn runtime_snapshot(&self) -> RequestTabRuntime {
        RequestTabRuntime {
            request_pane: self.request_pane,
            response_tab: self.response_tab,
            pretty_body: self.pretty_body,
            response: self.response.clone(),
            response_request: self.response_request.clone(),
            response_sensitive_values: self.response_sensitive_values.clone(),
            request_error: self.request_error.clone(),
            script_diagnostic: self.script_diagnostic.clone(),
            pre_script_report: self.pre_script_report.clone(),
            post_script_report: self.post_script_report.clone(),
            script_console_reports: self.script_console_reports.clone(),
            script_console_hidden_rows: self.script_console_hidden_rows,
            preview_error: self.preview_error.clone(),
            copied: self.copied,
            request_notice: self.request_notice.clone(),
        }
    }
}

impl ApiTester {
    fn pane_editor_mut(&mut self, pane_id: PaneId) -> Option<&mut PaneEditorState> {
        self.pane_editors.get_mut(&pane_id)
    }

    /// Read the live editor method, including tabs hosted in secondary panes.
    pub(super) fn request_tab_method(&self, tab: &RequestTabRecord, cx: &App) -> String {
        if tab.template().is_websocket() {
            return "WS".to_owned();
        }
        let method = if self.request_tabs.active_tab_id() == tab.id() {
            self.method.read(cx).value().to_string()
        } else if let Some(session) = self
            .pane_editors
            .values()
            .find(|session| session.active_tab_id.as_ref() == Some(tab.id()))
        {
            session.method.read(cx).value().to_string()
        } else {
            tab.template().request.method.clone()
        };
        method.trim().to_ascii_uppercase()
    }

    /// Persist the editor contents of every request currently shown outside
    /// the primary pane before a server snapshot is reconciled.
    pub(super) fn snapshot_secondary_pane_request_tabs(&mut self, cx: &App) {
        let active_workspace_tab = self.workspace_tabs.active_tab(&self.request_tabs);
        let primary_pane_id = self.panes.pane_for_tab(&active_workspace_tab);
        let snapshots = self
            .pane_editors
            .iter()
            .filter_map(|(pane_id, session)| {
                if primary_pane_id == Some(*pane_id) {
                    return None;
                }
                let tab_id = session.active_tab_id.as_ref()?;
                let active_tab = self.panes.pane(*pane_id)?.active_tab()?;
                if active_tab != WorkspaceTab::Request(tab_id.clone()) {
                    return None;
                }
                Some((
                    tab_id.clone(),
                    session.snapshot_template(cx),
                    session.runtime_snapshot(),
                ))
            })
            .collect::<Vec<_>>();

        for (tab_id, template, runtime) in snapshots {
            if let Some(record) = self.request_tabs.get_mut(&tab_id) {
                record.set_template(template);
            }
            self.request_tab_runtime
                .insert(tab_id.as_str().to_owned(), runtime);
        }
    }

    /// Reload clean secondary editors from the new server snapshot while
    /// retaining local drafts in dirty editors.
    pub(super) fn refresh_secondary_pane_request_tabs(
        &mut self,
        conflicts: &HashSet<RequestTabId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active_workspace_tab = self.workspace_tabs.active_tab(&self.request_tabs);
        let primary_pane_id = self.panes.pane_for_tab(&active_workspace_tab);
        let refreshes = self
            .pane_editors
            .iter()
            .filter_map(|(pane_id, session)| {
                if primary_pane_id == Some(*pane_id) {
                    return None;
                }
                let tab_id = session.active_tab_id.as_ref()?;
                let active_tab = self.panes.pane(*pane_id)?.active_tab()?;
                if active_tab != WorkspaceTab::Request(tab_id.clone()) {
                    return None;
                }
                let record = self.request_tabs.get(tab_id)?.clone();
                let runtime = self
                    .request_tab_runtime
                    .get(tab_id.as_str())
                    .cloned()
                    .unwrap_or_default();
                Some((*pane_id, tab_id.clone(), record, runtime))
            })
            .collect::<Vec<_>>();

        for (pane_id, tab_id, record, mut runtime) in refreshes {
            if conflicts.contains(&tab_id) {
                runtime.request_notice = Some(
                    "This request changed on the server. Your edits are still here.".to_owned(),
                );
                self.request_tab_runtime
                    .insert(tab_id.as_str().to_owned(), runtime.clone());
            }
            let Some(session) = self.pane_editors.get_mut(&pane_id) else {
                continue;
            };
            if !record.is_dirty() {
                session.load_template(
                    pane_id,
                    record.template(),
                    &runtime,
                    &self.settings.formatter,
                    window,
                    cx,
                );
            } else if conflicts.contains(&tab_id) {
                session.request_notice = runtime.request_notice.clone();
            }
        }
    }

    /// Build (or refresh) the request-editor session for `pane_id` so it tracks
    /// the pane's active request tab, and drop sessions for panes that no
    /// longer exist.
    pub(super) fn reconcile_pane_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active_workspace_tab = self.workspace_tabs.active_tab(&self.request_tabs);
        let primary_pane_id = self.panes.pane_for_tab(&active_workspace_tab);
        // A pane session is only a secondary-surface cache. Keeping it after
        // that pane becomes primary lets its stale runtime win if the pane is
        // demoted again, which used to make responses disappear on tab changes.
        if let Some(primary_pane_id) = primary_pane_id {
            self.pane_editors.remove(&primary_pane_id);
        }
        let mut live = std::collections::HashSet::new();
        let pane_ids = self
            .panes
            .panes()
            .iter()
            .map(|pane| pane.id())
            .collect::<Vec<_>>();
        for pane_id in pane_ids {
            live.insert(pane_id);
            if primary_pane_id == Some(pane_id) {
                continue;
            }
            let active_tab = self.panes.pane(pane_id).and_then(|pane| pane.active_tab());
            let Some(WorkspaceTab::Request(tab_id)) = active_tab else {
                continue;
            };
            self.ensure_pane_editor_for(pane_id, tab_id, window, cx);
        }
        self.pane_editors
            .retain(|pane_id, _| live.contains(pane_id));
    }

    pub(super) fn ensure_pane_editor_for(
        &mut self,
        pane_id: PaneId,
        tab_id: RequestTabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let record = match self.request_tabs.get(&tab_id) {
            Some(record) => record.clone(),
            None => return,
        };
        let runtime = self
            .request_tab_runtime
            .get(tab_id.as_str())
            .cloned()
            .unwrap_or_default();
        let needs_reload = match self.pane_editors.get(&pane_id) {
            None => true,
            Some(session) => session.active_tab_id.as_ref() != Some(&tab_id),
        };
        // When a pane's active request tab is replaced by a different one,
        // persist the outgoing session's contents back into the global request
        // records so the moved tab keeps the edits it had in this pane.
        if needs_reload {
            if let Some(old) = self.pane_editors.get(&pane_id)
                && let Some(old_id) = old.active_tab_id.clone()
                && old_id != tab_id
            {
                let template = old.snapshot_template(cx);
                let runtime = old.runtime_snapshot();
                if let Some(record) = self.request_tabs.get_mut(&old_id) {
                    record.set_template(template);
                }
                self.request_tab_runtime
                    .insert(old_id.as_str().to_owned(), runtime);
            }
            let mut session = PaneEditorState::new(pane_id, tab_id.clone(), window, cx);
            session.load_template(
                pane_id,
                record.template(),
                &runtime,
                &self.settings.formatter,
                window,
                cx,
            );
            // Secondary-pane editors are built with defaults; apply the
            // persisted editor preferences so they match the primary surface.
            let editor_settings = self.settings.editor.clone();
            for editor in session.code_editors() {
                editor.update(cx, |editor, cx| {
                    editor.apply_editor_settings(&editor_settings, window, cx);
                });
            }
            session.active_tab_id = Some(tab_id);
            self.pane_editors.insert(pane_id, session);
        }
    }
}

impl PaneEditorState {
    fn dom_key(&self, pane_id: PaneId) -> SharedString {
        format!("pane-{}", pane_id.0).into()
    }

    /// The session's code editors, for applying persisted editor settings.
    pub(in crate::app) fn code_editors(&self) -> Vec<Entity<CodeEditor>> {
        vec![
            self.body.clone(),
            self.documentation.clone(),
            self.pre_request_script.clone(),
            self.post_response_script.clone(),
            self.response_editor.clone(),
        ]
    }
}

impl ApiTester {
    /// Render the content of a non-primary pane: a real request editor when the
    /// pane's active tab is a request, the welcome page for a welcome tab, and
    /// a lightweight placeholder for tool tabs.
    pub(super) fn render_secondary_pane_content(
        &self,
        pane: &Pane,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane_id = pane.id();
        let inner = match pane.active_tab() {
            Some(WorkspaceTab::Request(_)) if self.pane_editors.contains_key(&pane_id) => {
                let key = pane_id.0;
                v_flex()
                    .size_full()
                    .min_h_0()
                    .child(
                        v_resizable(SharedString::from(format!(
                            "secondary-request-response-split-{key}"
                        )))
                        .child(
                            resizable_panel()
                                .size(px(420.))
                                .size_range(px(300.)..px(900.))
                                .child(self.render_pane_request_panel(pane_id, cx)),
                        )
                        .child(
                            resizable_panel()
                                .size_range(px(200.)..px(1_400.))
                                .child(self.render_pane_response_panel(pane_id, cx)),
                        ),
                    )
                    .into_any_element()
            }
            Some(WorkspaceTab::Welcome) => div()
                .size_full()
                .min_h_0()
                .child(self.render_welcome_page(cx))
                .into_any_element(),
            Some(WorkspaceTab::Tool(_)) | Some(WorkspaceTab::Request(_)) | None => {
                self.render_secondary_placeholder(pane_id)
            }
        };

        let insert_index = pane.tabs().len();
        v_flex()
            .size_full()
            .flex_1()
            .min_h_0()
            .relative()
            .can_drop(move |value, _, _| value.downcast_ref::<WorkspaceTabDrag>().is_some())
            .drag_over::<WorkspaceTabDrag>(move |style, _, _, cx| {
                style.bg(cx.theme().drop_target.opacity(0.35))
            })
            .on_drop(
                cx.listener(move |this, drag: &WorkspaceTabDrag, window, cx| {
                    this.on_workspace_tab_move(drag, pane_id, insert_index, window, cx);
                }),
            )
            .child(inner)
            .into_any_element()
    }

    fn render_secondary_placeholder(&self, pane_id: PaneId) -> AnyElement {
        let _ = pane_id;
        v_flex()
            .size_full()
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_center()
            .gap_2()
            .child(div().text_sm().child("Tool surface"))
            .child(
                div()
                    .text_xs()
                    .child("This pane hosts a tool tab. Move a request here to edit it."),
            )
            .into_any_element()
    }

    fn render_pane_request_panel(&self, pane_id: PaneId, cx: &mut Context<Self>) -> AnyElement {
        let Some(session) = self.pane_editors.get(&pane_id) else {
            return self.render_secondary_placeholder(pane_id);
        };
        documentation::refresh_targets(
            &session.documentation_intelligence,
            &session.documentation,
            &session.query_params,
            &session.headers,
            Some((session.body_mode, session.raw_body_language, &session.body)),
            &self.workspace,
            cx,
        );
        let header_count = session
            .headers
            .iter()
            .filter(|row| row.enabled && !input_text_is_blank(&row.name, cx))
            .count();
        let query_param_count = session
            .query_params
            .iter()
            .filter(|row| {
                row.enabled
                    && (!input_text_is_blank(&row.key, cx) || !input_text_is_blank(&row.value, cx))
            })
            .count();
        let key = session.dom_key(pane_id);

        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.api_surface())
            .child(
                v_flex()
                    .flex_shrink_0()
                    .gap_3()
                    .pt_4()
                    .child(
                        div()
                            .px_4()
                            .child(self.render_pane_url_row(session, pane_id, cx)),
                    )
                    .child(
                        TabBar::new(SharedString::from(format!("{key}-request-tabs")))
                            .underline()
                            .px_4()
                            .children([
                                format!("Params ({query_param_count})"),
                                format!("Headers ({header_count})"),
                                "Body".to_owned(),
                                "Pre-request".to_owned(),
                                "Post-response".to_owned(),
                                self.cookie_tab_label(),
                                "Documentation".to_owned(),
                            ])
                            .selected_index(session.request_pane.index())
                            .on_click(cx.listener(move |this, index: &usize, _, cx| {
                                this.pane_set_request_pane(
                                    pane_id,
                                    RequestPane::from_index(*index),
                                    cx,
                                );
                            })),
                    ),
            )
            .child(request_workspace::request_content_container(
                div()
                    .size_full()
                    .pt_2()
                    .bg(
                        if matches!(
                            session.request_pane,
                            RequestPane::Params | RequestPane::Headers | RequestPane::Cookies
                        ) {
                            cx.api_surface_low()
                        } else {
                            cx.api_surface()
                        },
                    )
                    .when(session.request_pane == RequestPane::Documentation, |this| {
                        this.child(session.documentation.clone())
                    })
                    .when(session.request_pane == RequestPane::Params, |this| {
                        this.child(self.render_pane_query_params_editor(session, pane_id, cx))
                    })
                    .when(session.request_pane == RequestPane::Headers, |this| {
                        this.child(self.render_pane_headers_editor(session, pane_id, cx))
                    })
                    .when(session.request_pane == RequestPane::Body, |this| {
                        this.child(self.render_pane_body_editor(session, pane_id, cx))
                    })
                    .when(session.request_pane == RequestPane::PreRequest, |this| {
                        this.child(session.pre_request_script.clone())
                    })
                    .when(session.request_pane == RequestPane::Cookies, |this| {
                        this.child(self.render_cookie_manager(format!("{key}-cookies").into(), cx))
                    })
                    .when(session.request_pane == RequestPane::PostResponse, |this| {
                        this.child(session.post_response_script.clone())
                    }),
            ))
            .into_any_element()
    }

    fn render_pane_url_row(
        &self,
        session: &PaneEditorState,
        pane_id: PaneId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let method = session.method.read(cx).value().trim().to_ascii_uppercase();
        let color = method_color(&method, cx);
        let key = session.dom_key(pane_id);
        let send_button_id: SharedString = format!("{key}-send-request").into();
        let sending = session
            .active_tab_id
            .as_ref()
            .is_some_and(|tab_id| self.pane_requests_in_flight.contains_key(tab_id.as_str()));

        h_flex()
            .w_full()
            .h(px(44.))
            .gap_3()
            .child(
                h_flex()
                    .h_full()
                    .flex_1()
                    .min_w_0()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.api_outline_variant())
                    .bg(cx.api_surface_container())
                    .overflow_hidden()
                    .child(
                        div()
                            .w(px(148.))
                            .h_full()
                            .flex_shrink_0()
                            .border_r_1()
                            .border_color(cx.api_outline_variant())
                            .bg(color.opacity(0.08))
                            .child(
                                Input::new(&session.method)
                                    .appearance(false)
                                    .large()
                                    .w(px(148.))
                                    .font_semibold()
                                    .text_color(color),
                            ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("{key}-url")))
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&session.url).appearance(false).large()),
                    ),
            )
            .child(
                Button::new(send_button_id)
                    .label(if sending { "Sending…" } else { "Send" })
                    .large()
                    .h(px(44.))
                    .rounded(px(12.))
                    .primary()
                    .disabled(sending)
                    .debug_selector(|| "secondary-pane-send-request".to_owned())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.start_pane_request(pane_id, window, cx);
                    })),
            )
            .into_any_element()
    }

    /// Shared chrome for an ordered row editor (headers or body fields): the
    /// bordered container, title bar, optional column-heading row, scrollable
    /// row list and add-row footer. The two editors differ only in their
    /// title, optional column headings, rows and add control, so the
    /// scaffolding lives here once rather than being copied per editor.
    fn render_row_editor(
        &self,
        scroll_id: SharedString,
        title: impl IntoElement,
        columns: Option<AnyElement>,
        rows: Vec<AnyElement>,
        add_control: impl IntoElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut editor = v_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .bg(cx.api_surface())
            .child(
                h_flex()
                    .h(px(42.))
                    .w_full()
                    .flex_shrink_0()
                    .px_3()
                    .justify_between()
                    .bg(cx.api_surface_low())
                    .child(title),
            );
        if let Some(columns) = columns {
            editor = editor.child(columns);
        }
        editor
            .child(
                v_flex()
                    .id(scroll_id)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows)
                    .child(
                        h_flex()
                            .h(px(42.))
                            .flex_shrink_0()
                            .px_3()
                            .border_t_1()
                            .border_color(cx.api_outline_variant())
                            .child(add_control),
                    ),
            )
            .into_any_element()
    }

    fn render_pane_query_params_editor(
        &self,
        session: &PaneEditorState,
        pane_id: PaneId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = session.dom_key(pane_id);
        let enabled_count = session
            .query_params
            .iter()
            .filter(|row| {
                row.enabled
                    && (!input_text_is_blank(&row.key, cx) || !input_text_is_blank(&row.value, cx))
            })
            .count();
        let columns = h_flex()
            .h(px(34.))
            .w_full()
            .flex_shrink_0()
            .bg(cx.api_surface_low())
            .text_xs()
            .font_semibold()
            .text_color(cx.theme().muted_foreground)
            .child(div().w(px(44.)))
            .child(pane_query_param_heading("KEY", cx))
            .child(pane_query_param_heading("VALUE", cx))
            .child(pane_query_param_heading("DESCRIPTION", cx))
            .child(div().w(px(44.)));
        let rows =
            session
                .query_params
                .iter()
                .map(|row| {
                    let id = row.id;
                    let description = documentation::explanation(
                        &session.documentation_intelligence,
                        &session.documentation,
                        crate::documentation_intelligence::TargetKind::Query,
                        row.key.read(cx).value().as_ref(),
                        cx,
                    );
                    h_flex()
                        .id(SharedString::from(format!("{key}-query-param-{id}")))
                        .w_full()
                        .h(px(44.))
                        .flex_shrink_0()
                        .border_t_1()
                        .border_color(cx.api_outline_variant())
                        .when(!row.enabled, |this| this.opacity(0.55))
                        .child(
                            div()
                                .w(px(44.))
                                .h_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Checkbox::new(SharedString::from(format!(
                                        "{key}-query-param-enabled-{id}"
                                    )))
                                    .checked(row.enabled)
                                    .small()
                                    .on_click(cx.listener(
                                        move |this, checked: &bool, window, cx| {
                                            this.pane_toggle_query_param(
                                                pane_id, id, *checked, window, cx,
                                            );
                                        },
                                    )),
                                ),
                        )
                        .child(
                            pane_query_param_input(&row.key, cx)
                                .id(SharedString::from(format!("{key}-query-key-description-{id}")))
                                .when_some(description.clone(), |this, description| {
                                    this.hoverable_tooltip(documentation::explanation_tooltip(
                                        description,
                                        cx.entity().downgrade(),
                                    ))
                                }),
                        )
                        .child(pane_query_param_input(&row.value, cx))
                        .child(documentation::description_cell(
                            format!("{key}-query-description-{id}").into(),
                            description,
                            cx,
                        ))
                        .child(
                            div()
                                .w(px(44.))
                                .h_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "{key}-delete-query-param-{id}"
                                    )))
                                    .icon(IconName::Delete)
                                    .xsmall()
                                    .ghost()
                                    .tooltip("Delete parameter")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.pane_remove_query_param(pane_id, id, window, cx);
                                    })),
                                ),
                        )
                        .into_any_element()
                })
                .collect::<Vec<_>>();
        let add_control = Button::new(SharedString::from(format!("{key}-add-query-param")))
            .icon(IconName::Plus)
            .label("Add parameter")
            .small()
            .ghost()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.pane_push_query_param(pane_id, window, cx);
            }));

        self.render_row_editor(
            SharedString::from(format!("{key}-query-params-scroll")),
            h_flex()
                .gap_2()
                .child(div().text_sm().font_semibold().child("Query Params"))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{enabled_count} enabled")),
                ),
            Some(columns.into_any_element()),
            rows,
            add_control,
            cx,
        )
    }

    fn render_pane_headers_editor(
        &self,
        session: &PaneEditorState,
        pane_id: PaneId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = session.dom_key(pane_id);
        let rows = session
            .headers
            .iter()
            .map(|row| self.render_pane_header_row(session, row, pane_id, cx))
            .collect::<Vec<_>>();
        let enabled_count = session
            .headers
            .iter()
            .filter(|row| row.enabled && !input_text_is_blank(&row.name, cx))
            .count();

        let columns = h_flex()
            .h(px(34.))
            .w_full()
            .flex_shrink_0()
            .bg(cx.api_surface_low())
            .text_xs()
            .font_semibold()
            .text_color(cx.theme().muted_foreground)
            .child(div().w(px(44.)).child(""))
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .px_3()
                    .flex()
                    .items_center()
                    .child("KEY"),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .px_3()
                    .flex()
                    .items_center()
                    .child("VALUE"),
            )
            .child(
                div()
                    .w(px(64.))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child("SHARE"),
            )
            .child(div().w(px(44.)));

        let add_control = Button::new("add-pane-header")
            .icon(IconName::Plus)
            .label("Add header")
            .small()
            .ghost()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.pane_push_header(pane_id, window, cx);
            }));

        self.render_row_editor(
            SharedString::from(format!("{key}-headers-scroll")),
            div()
                .text_sm()
                .font_semibold()
                .child(format!("{enabled_count} enabled")),
            Some(columns.into_any_element()),
            rows,
            add_control,
            cx,
        )
    }

    /// Shared per-row layout for the header and body-field editors: an
    /// enabled checkbox, an optional kind/flag cell, the name/value inputs and
    /// the delete control. Both row types render identically except for the
    /// middle cell and their toggle/remove actions, so the scaffolding lives
    /// here once.
    #[allow(clippy::too_many_arguments)]
    fn render_pane_row(
        &self,
        row_id: SharedString,
        height: f32,
        enabled: bool,
        enabled_id: SharedString,
        delete_id: SharedString,
        delete_tooltip: &'static str,
        middle: Option<AnyElement>,
        name_input: impl IntoElement,
        value_input: impl IntoElement,
        on_toggle: impl Fn(&mut Self, bool, &mut Window, &mut Context<Self>) + 'static,
        on_delete: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut row = h_flex()
            .id(row_id)
            .w_full()
            .h(px(height))
            .flex_shrink_0()
            .border_t_1()
            .border_color(cx.api_outline_variant())
            .bg(cx.api_surface())
            .when(!enabled, |this| this.opacity(0.55))
            .child(
                div()
                    .w(px(44.))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Checkbox::new(enabled_id).checked(enabled).small().on_click(
                        cx.listener(move |this, checked: &bool, window, cx| {
                            on_toggle(this, *checked, window, cx);
                        }),
                    )),
            );
        if let Some(middle) = middle {
            row = row.child(middle);
        }
        row.child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .border_l_1()
                .border_color(cx.api_outline_variant())
                .child(name_input),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .border_l_1()
                .border_color(cx.api_outline_variant())
                .child(value_input),
        )
        .child(
            div()
                .w(px(44.))
                .h_full()
                .flex_shrink_0()
                .border_l_1()
                .border_color(cx.api_outline_variant())
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Button::new(delete_id)
                        .icon(IconName::Delete)
                        .xsmall()
                        .ghost()
                        .tooltip(delete_tooltip)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            on_delete(this, window, cx);
                        })),
                ),
        )
        .into_any_element()
    }

    fn render_pane_header_row(
        &self,
        session: &PaneEditorState,
        row: &HeaderRow,
        pane_id: PaneId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = row.id;
        let key = session.dom_key(pane_id);
        let description = documentation::explanation(
            &session.documentation_intelligence,
            &session.documentation,
            crate::documentation_intelligence::TargetKind::Header,
            row.name.read(cx).value().trim(),
            cx,
        );

        let middle = div()
            .id(SharedString::from(format!("{key}-header-shared-cell-{id}")))
            .w(px(64.))
            .h_full()
            .flex_shrink_0()
            .border_l_1()
            .border_color(cx.api_outline_variant())
            .flex()
            .items_center()
            .justify_center()
            .tooltip(|window, cx| {
                Tooltip::new("Include this header in server-shared history").build(window, cx)
            })
            .child(
                Checkbox::new(SharedString::from(format!("{key}-header-shared-{id}")))
                    .checked(row.shared)
                    .small()
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        this.pane_toggle_header_sharing(pane_id, id, *checked, cx);
                    })),
            );

        self.render_pane_row(
            SharedString::from(format!("{key}-header-row-{id}")),
            44.,
            row.enabled,
            SharedString::from(format!("{key}-header-enabled-{id}")),
            SharedString::from(format!("{key}-header-delete-{id}")),
            "Delete header",
            Some(middle.into_any_element()),
            div()
                .id(SharedString::from(format!("{key}-header-description-{id}")))
                .size_full()
                .when_some(description, |this, description| {
                    this.hoverable_tooltip(documentation::explanation_tooltip(
                        description,
                        cx.entity().downgrade(),
                    ))
                })
                .child(Input::new(&row.name).appearance(false).small().size_full().px_3()),
            Input::new(&row.value)
                .appearance(false)
                .small()
                .size_full()
                .px_3(),
            move |this, enabled, _, cx| this.pane_toggle_header(pane_id, id, enabled, cx),
            move |this, window, cx| this.pane_remove_header(pane_id, id, window, cx),
            cx,
        )
    }

    fn render_pane_body_editor(
        &self,
        session: &PaneEditorState,
        pane_id: PaneId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let content = match session.body_mode {
            BodyMode::None => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(cx.theme().muted_foreground)
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child("This request has no body"),
                )
                .into_any_element(),
            BodyMode::Raw => div()
                .size_full()
                .child(session.body.clone())
                .into_any_element(),
            BodyMode::FormUrlEncoded => self.render_pane_body_fields(session, pane_id, false, cx),
            BodyMode::MultipartFormData => self.render_pane_body_fields(session, pane_id, true, cx),
        };

        v_flex()
            .size_full()
            .min_h_0()
            .child(
                div()
                    .flex_shrink_0()
                    .px_3()
                    .pb_2()
                    .child(self.render_pane_body_mode_toolbar(session, pane_id, cx)),
            )
            .child(request_workspace::request_content_container(content))
            .into_any_element()
    }

    fn render_pane_body_mode_toolbar(
        &self,
        session: &PaneEditorState,
        pane_id: PaneId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mode_switch = h_flex()
            .flex_shrink_0()
            .p(px(2.))
            .rounded(px(6.))
            .bg(cx.api_surface_container())
            .children(
                BodyMode::all()
                    .iter()
                    .copied()
                    .enumerate()
                    .map(|(index, mode)| {
                        let selected = session.body_mode == mode;
                        Button::new(SharedString::from(format!(
                            "pane-{}-body-mode-{index}",
                            pane_id.0
                        )))
                        .label(mode.label())
                        .small()
                        .ghost()
                        .h(px(24.))
                        .px(px(10.))
                        .rounded(px(4.))
                        .text_color(if selected {
                            cx.theme().foreground
                        } else {
                            cx.theme().muted_foreground
                        })
                        .when(selected, |button| {
                            button
                                .bg(if cx.api_surface().l < 0.5 {
                                    cx.api_surface_highest()
                                } else {
                                    cx.api_surface_lowest()
                                })
                                .border_1()
                                .border_color(cx.api_outline_variant())
                                .shadow_xs()
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.pane_select_body_mode(pane_id, mode, cx);
                        }))
                    }),
            );

        let key = session.dom_key(pane_id);
        let selected_language = session.raw_body_language;
        let owner = cx.entity().downgrade();
        h_flex()
            .w_full()
            .flex_wrap()
            .justify_between()
            .gap_2()
            .child(mode_switch)
            .when(session.body_mode == BodyMode::Raw, |this| {
                this.child(
                    Button::new(SharedString::from(format!("{key}-raw-language")))
                        .label(selected_language.label())
                        .dropdown_caret(true)
                        .small()
                        .outline()
                        .rounded(px(18.))
                        .dropdown_menu(move |menu, _, _| {
                            RawBodyLanguage::all().iter().copied().fold(
                                menu.min_w(px(190.)).max_h(px(420.)).scrollable(true),
                                |menu, language| {
                                    let owner = owner.clone();
                                    menu.item(
                                        PopupMenuItem::new(language.label())
                                            .checked(language == selected_language)
                                            .on_click(move |_, _, cx| {
                                                if let Some(this) = owner.upgrade() {
                                                    this.update(cx, |this, cx| {
                                                        this.pane_select_raw_language(
                                                            pane_id, language, cx,
                                                        );
                                                    });
                                                }
                                            }),
                                    )
                                },
                            )
                        }),
                )
            })
            .into_any_element()
    }

    fn render_pane_body_fields(
        &self,
        session: &PaneEditorState,
        pane_id: PaneId,
        multipart: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = session
            .body_fields
            .iter()
            .map(|row| self.render_pane_body_field_row(session, row, pane_id, multipart, cx))
            .collect::<Vec<_>>();
        let key = session.dom_key(pane_id);

        let add_control = Button::new("add-pane-body-field")
            .icon(IconName::Plus)
            .label("Add field")
            .small()
            .ghost()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.pane_push_body_field(pane_id, window, cx);
            }));

        self.render_row_editor(
            SharedString::from(format!("{key}-body-fields-scroll")),
            div().text_sm().font_semibold().child(if multipart {
                "form-data"
            } else {
                "x-www-form-urlencoded"
            }),
            None,
            rows,
            add_control,
            cx,
        )
    }

    fn render_pane_body_field_row(
        &self,
        session: &PaneEditorState,
        row: &request_body_editor::BodyFieldRow,
        pane_id: PaneId,
        multipart: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = row.id;
        let key = session.dom_key(pane_id);
        let selected_kind = row.kind;
        let kind_owner = cx.entity().downgrade();

        let middle = multipart.then(|| {
            div()
                .w(px(96.))
                .h_full()
                .flex_shrink_0()
                .border_l_1()
                .border_color(cx.api_outline_variant())
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Button::new(SharedString::from(format!("{key}-body-field-kind-{id}")))
                        .label(selected_kind.label())
                        .dropdown_caret(true)
                        .xsmall()
                        .ghost()
                        .w(px(82.))
                        .dropdown_menu(move |menu, _, _| {
                            let owner = kind_owner.clone();
                            BodyFieldKind::all().iter().copied().fold(
                                menu.min_w(px(150.)),
                                |menu, kind| {
                                    let owner = owner.clone();
                                    menu.item(
                                        PopupMenuItem::new(kind.label())
                                            .checked(kind == selected_kind)
                                            .on_click(move |_, _, cx| {
                                                if let Some(this) = owner.upgrade() {
                                                    this.update(cx, |this, cx| {
                                                        this.pane_set_body_field_kind(
                                                            pane_id, id, kind, cx,
                                                        );
                                                    });
                                                }
                                            }),
                                    )
                                },
                            )
                        }),
                )
                .into_any_element()
        });

        self.render_pane_row(
            SharedString::from(format!("{key}-body-field-row-{id}")),
            46.,
            row.enabled,
            SharedString::from(format!("{key}-body-field-enabled-{id}")),
            SharedString::from(format!("{key}-body-field-delete-{id}")),
            "Delete field",
            middle,
            Input::new(&row.name)
                .appearance(false)
                .small()
                .size_full()
                .px_3(),
            Input::new(&row.value)
                .appearance(false)
                .small()
                .size_full()
                .px_3(),
            move |this, enabled, _, cx| this.pane_toggle_body_field(pane_id, id, enabled, cx),
            move |this, window, cx| this.pane_remove_body_field(pane_id, id, window, cx),
            cx,
        )
    }

    fn render_pane_response_panel(&self, pane_id: PaneId, cx: &mut Context<Self>) -> AnyElement {
        let Some(session) = self.pane_editors.get(&pane_id) else {
            return self.render_secondary_placeholder(pane_id);
        };
        let key = session.dom_key(pane_id);
        let Some(response) = &session.response else {
            let state_label = if session.script_diagnostic.is_some() {
                "Script failed"
            } else if session.request_error.is_some() {
                "Request failed"
            } else {
                "No response yet"
            };
            return v_flex()
                .size_full()
                .min_h_0()
                .bg(cx.api_surface())
                .child(
                    h_flex()
                        .h(px(56.))
                        .flex_shrink_0()
                        .px_4()
                        .gap_3()
                        .border_b_1()
                        .border_color(cx.api_outline_variant())
                        .child(div().text_base().font_semibold().child("Response"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(state_label),
                        ),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_h_0()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .text_color(cx.theme().muted_foreground)
                        .child(div().text_sm().child(state_label)),
                )
                .into_any_element();
        };

        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.api_surface())
            .child(
                h_flex()
                    .h(px(56.))
                    .flex_shrink_0()
                    .px_4()
                    .gap_4()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.api_outline_variant())
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_4()
                            .child(div().text_base().font_semibold().child("Response"))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(status_color(response.status, cx))
                                    .child(format!("{} {}", response.status, response.status_text)),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("pane-toggle-pretty")
                                    .label(if session.pretty_body { "Pretty" } else { "Raw" })
                                    .small()
                                    .ghost()
                                    .rounded(px(18.))
                                    .selected(session.pretty_body)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.pane_toggle_pretty(pane_id, window, cx);
                                    })),
                            )
                            .child(
                                Button::new("pane-copy-response")
                                    .label(if session.copied { "Copied" } else { "Copy" })
                                    .small()
                                    .ghost()
                                    .rounded(px(18.))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.pane_copy_response(pane_id, cx);
                                    })),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .h(px(42.))
                    .flex_shrink_0()
                    .px_4()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.api_outline_variant())
                    .child(
                        TabBar::new(SharedString::from(format!("{key}-response-tabs")))
                            .underline()
                            .children(["Body", "Headers", "Preview", "Scripts"])
                            .selected_index(session.response_tab.index())
                            .on_click(cx.listener(move |this, index: &usize, _, cx| {
                                this.pane_set_response_tab(pane_id, *index, cx);
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .ml_3()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_right()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(compact_url(&response.final_url)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .when(session.response_tab != ResponseTab::Body, |this| this.p_4())
                    .when(session.response_tab == ResponseTab::Body, |this| {
                        this.child(self.render_pane_response_body(session, pane_id, cx))
                    })
                    .when(session.response_tab == ResponseTab::Headers, |this| {
                        this.child(self.render_pane_response_headers(session, pane_id, cx))
                    })
                    .when(session.response_tab == ResponseTab::Preview, |this| {
                        this.child(
                            v_flex()
                                .size_full()
                                .items_center()
                                .justify_center()
                                .text_color(cx.theme().muted_foreground)
                                .child(
                                    div().text_sm().child(
                                        "Live preview is not available in a secondary pane.",
                                    ),
                                ),
                        )
                    })
                    .when(session.response_tab == ResponseTab::Scripts, |this| {
                        this.child(
                            v_flex()
                                .size_full()
                                .items_center()
                                .justify_center()
                                .text_color(cx.theme().muted_foreground)
                                .child(div().text_sm().child("No script output recorded.")),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_pane_response_body(
        &self,
        session: &PaneEditorState,
        pane_id: PaneId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = session.dom_key(pane_id);
        let Some(_) = &session.response else {
            return div().size_full().into_any_element();
        };
        div()
            .id(SharedString::from(format!("{key}-response-body")))
            .debug_selector(|| "secondary-pane-response-code-editor".to_owned())
            .size_full()
            .min_h_0()
            .bg(cx.api_surface_lowest())
            .child(session.response_editor.clone())
            .into_any_element()
    }

    fn render_pane_response_headers(
        &self,
        session: &PaneEditorState,
        pane_id: PaneId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = session.dom_key(pane_id);
        let Some(response) = &session.response else {
            return div().size_full().into_any_element();
        };
        v_flex()
            .id(SharedString::from(format!("{key}-response-headers")))
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .children(response.headers.iter().map(|header| {
                h_flex()
                    .w_full()
                    .flex_shrink_0()
                    .px_3()
                    .py_1()
                    .gap_3()
                    .child(
                        div()
                            .w(px(220.))
                            .flex_shrink_0()
                            .text_sm()
                            .font_semibold()
                            .child(header.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(header.value.clone()),
                    )
            }))
            .into_any_element()
    }
}

fn pane_query_param_heading(label: &'static str, cx: &App) -> impl IntoElement {
    div()
        .flex_1()
        .min_w_0()
        .h_full()
        .px_3()
        .border_l_1()
        .border_color(cx.api_outline_variant())
        .flex()
        .items_center()
        .child(label)
}

fn pane_query_param_input(input: &Entity<InputState>, cx: &App) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .h_full()
        .border_l_1()
        .border_color(cx.api_outline_variant())
        .child(
            Input::new(input)
                .appearance(false)
                .small()
                .size_full()
                .px_3(),
        )
}

fn status_color(status: u16, cx: &App) -> Hsla {
    match status {
        200..=299 => cx.theme().success,
        400..=499 => cx.theme().warning,
        _ => cx.theme().danger,
    }
}

impl ApiTester {
    fn start_pane_request(&mut self, pane_id: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.pane_editors.get(&pane_id) else {
            return;
        };
        let Some(tab_id) = session.active_tab_id.clone() else {
            return;
        };
        if self.pane_requests_in_flight.contains_key(tab_id.as_str()) {
            return;
        }
        let template = session.snapshot_template(cx);
        if let Some(record) = self.request_tabs.get_mut(&tab_id) {
            record.set_template(template.clone());
        } else {
            return;
        }

        let validation_error = if template.request.method.trim().is_empty() {
            Some("HTTP method cannot be empty.".to_owned())
        } else if self.active_environment_editor_is_dirty(cx) {
            Some(
                "The active environment has unsaved changes. Save or Revert them before sending."
                    .to_owned(),
            )
        } else {
            None
        };
        if let Some(message) = validation_error {
            self.finish_pane_request_error(&tab_id, message, window, cx);
            return;
        }

        let environment = self.workspace.active_environment();
        let mut resolved = match resolve_request(&template.request, environment) {
            Ok(resolved) => resolved,
            Err(error) => {
                self.finish_pane_request_error(&tab_id, error.to_string(), window, cx);
                return;
            }
        };
        if let Some(environment) = environment {
            resolved.sensitive_values.extend(
                environment
                    .variables
                    .iter()
                    .filter(|variable| variable.enabled && variable.secret)
                    .map(|variable| variable.value.clone()),
            );
        }

        self.pane_request_generation = self.pane_request_generation.wrapping_add(1);
        let generation = self.pane_request_generation;
        self.pane_requests_in_flight
            .insert(tab_id.as_str().to_owned(), generation);
        if let Some(session) = self.pane_editors.get_mut(&pane_id) {
            session.response = None;
            session.response_request = None;
            session.response_sensitive_values.clear();
            session.formatted_body = None;
            session.request_error = None;
            session.script_diagnostic = None;
            session.pre_script_report = None;
            session.post_script_report = None;
            session.script_console_reports.clear();
            session.script_console_hidden_rows = 0;
            session.preview_error = None;
            session.copied = false;
        }

        let request = resolved.request.clone();
        let task = match self.workspace_providers.active_id() {
            WorkspaceProviderId::Local(_) => {
                spawn_request(self.runtime.handle(), self.client.clone(), request)
            }
            WorkspaceProviderId::Upstream { .. } => {
                let target = match self.active_upstream_workspace() {
                    Ok(target) => target,
                    Err(error) => {
                        self.pane_requests_in_flight.remove(tab_id.as_str());
                        self.finish_pane_request_error(&tab_id, error.to_string(), window, cx);
                        return;
                    }
                };
                let vault = self.credential_vault.clone();
                let client = self.upstream_execution_client.clone();
                let local_client = self.client.clone();
                let cookie_jar = self.cookie_jar.clone();
                let runtime = Arc::clone(&self.runtime);
                let credential_upstream_id = target.upstream_id.clone();
                RequestTask::spawn(self.runtime.handle(), async move {
                    let credential = runtime
                        .spawn_blocking(move || vault.load_upstream(&credential_upstream_id))
                        .await
                        .map_err(|error| RequestError::TaskFailed(error.to_string()))?
                        .map_err(|error| RequestError::Upstream(error.to_string()))?
                        .ok_or_else(|| {
                            RequestError::Upstream("Log in to this server again.".to_owned())
                        })?;
                    if credential.expires_at <= Utc::now() {
                        return Err(RequestError::Upstream(
                            "Log in to this server again.".to_owned(),
                        ));
                    }
                    send_request_for_upstream_workspace(
                        &client,
                        &local_client,
                        &target.base_url,
                        credential.bearer_token(),
                        &target.workspace_id,
                        request,
                        cookie_jar.as_ref(),
                    )
                    .await
                })
            }
        };
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.wait().await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_pane_request(tab_id, generation, resolved, result, window, cx);
            });
        })
        .detach();
    }

    fn finish_pane_request_error(
        &mut self,
        tab_id: &RequestTabId,
        message: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let message = message;
        let runtime = self
            .request_tab_runtime
            .entry(tab_id.as_str().to_owned())
            .or_default();
        runtime.response = None;
        runtime.response_request = None;
        runtime.response_sensitive_values.clear();
        runtime.request_error = Some(message.clone());
        self.refresh_visible_pane_runtime(tab_id, window, cx);
    }

    fn finish_pane_request(
        &mut self,
        tab_id: RequestTabId,
        generation: u64,
        resolved: crate::core::ResolvedRequest,
        result: Result<ResponseData, RequestError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pane_requests_in_flight.get(tab_id.as_str()) != Some(&generation) {
            return;
        }
        self.pane_requests_in_flight.remove(tab_id.as_str());
        if self.request_tabs.get(&tab_id).is_none() {
            cx.notify();
            return;
        }

        let runtime = self
            .request_tab_runtime
            .entry(tab_id.as_str().to_owned())
            .or_default();
        runtime.response_request = Some(resolved.request.clone());
        runtime.response_sensitive_values = resolved.sensitive_values.clone();
        runtime.script_diagnostic = None;
        runtime.pre_script_report = None;
        runtime.post_script_report = None;
        runtime.script_console_reports.clear();
        runtime.script_console_hidden_rows = 0;
        runtime.preview_error = None;
        runtime.copied = false;
        match result {
            Ok(mut response) => {
                response.final_url = resolved.redact_secrets(&response.final_url);
                runtime.response = Some(response.clone());
                runtime.request_error = None;
                let history_entry = HistoryEntry::completed_with_secrets(
                    &resolved.request,
                    &response,
                    &resolved.sensitive_values,
                );
                self.history.push(history_entry);
                self.persist_history();
            }
            Err(error) => {
                runtime.response = None;
                runtime.request_error = Some(resolved.redact_secrets(&error.to_string()));
            }
        }
        self.refresh_visible_pane_runtime(&tab_id, window, cx);
        cx.notify();
    }

    fn refresh_visible_pane_runtime(
        &mut self,
        tab_id: &RequestTabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(record) = self.request_tabs.get(tab_id).cloned() else {
            return;
        };
        let runtime = self
            .request_tab_runtime
            .get(tab_id.as_str())
            .cloned()
            .unwrap_or_default();
        let pane_ids = self
            .pane_editors
            .iter()
            .filter_map(|(pane_id, session)| {
                (session.active_tab_id.as_ref() == Some(tab_id)).then_some(*pane_id)
            })
            .collect::<Vec<_>>();
        for pane_id in pane_ids {
            if let Some(session) = self.pane_editors.get_mut(&pane_id) {
                session.load_template(
                    pane_id,
                    record.template(),
                    &runtime,
                    &self.settings.formatter,
                    window,
                    cx,
                );
            }
        }
    }

    fn pane_set_request_pane(
        &mut self,
        pane_id: PaneId,
        pane: RequestPane,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.pane_editor_mut(pane_id) {
            session.request_pane = pane;
        }
        cx.notify();
    }

    fn pane_push_query_param(
        &mut self,
        pane_id: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.pane_editor_mut(pane_id) {
            session.push_query_param_row(pane_id, "", "", true, window, cx);
        }
        cx.notify();
    }

    fn pane_remove_query_param(
        &mut self,
        pane_id: PaneId,
        row_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.pane_editor_mut(pane_id) {
            session.query_params.retain(|row| row.id != row_id);
            if session.query_params.is_empty() {
                session.push_query_param_row(pane_id, "", "", true, window, cx);
            }
        }
        self.pane_sync_url_from_query_params(pane_id, window, cx);
    }

    fn pane_toggle_query_param(
        &mut self,
        pane_id: PaneId,
        row_id: usize,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.pane_editor_mut(pane_id)
            && let Some(row) = session.query_params.iter_mut().find(|row| row.id == row_id)
        {
            row.enabled = enabled;
        }
        self.pane_sync_url_from_query_params(pane_id, window, cx);
    }

    fn pane_sync_url_from_query_params(
        &mut self,
        pane_id: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.pane_editors.get(&pane_id) else {
            return;
        };
        let params = session.normalized_query_params(cx);
        let input = session.url.clone();
        let current = input.read(cx).value().to_string();
        let updated = url_with_query_params(&current, &params);
        if updated != current {
            input.update(cx, |state, cx| state.set_value(updated, window, cx));
        }
        cx.notify();
    }

    fn pane_sync_query_params_from_url(
        &mut self,
        pane_id: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.pane_editors.get(&pane_id) else {
            return;
        };
        let url = session.url.read(cx).value().to_string();
        let existing = session.normalized_query_params(cx);
        let incoming_params = query_params_from_url(&url);
        let current = existing
            .iter()
            .filter(|param| param.enabled)
            .map(|param| (param.key.clone(), param.value.clone()))
            .collect::<Vec<_>>();
        let incoming = incoming_params
            .iter()
            .map(|param| (param.key.clone(), param.value.clone()))
            .collect::<Vec<_>>();
        if current == incoming {
            return;
        }

        let mut reconciled = incoming_params;
        reconciled.extend(existing.into_iter().filter(|param| !param.enabled));

        let Some(session) = self.pane_editors.get_mut(&pane_id) else {
            return;
        };
        session.query_params.clear();
        for param in reconciled {
            session.push_query_param_row(
                pane_id,
                param.key,
                param.value,
                param.enabled,
                window,
                cx,
            );
        }
        if session.query_params.is_empty() {
            session.push_query_param_row(pane_id, "", "", true, window, cx);
        }
        cx.notify();
    }

    fn pane_push_header(&mut self, pane_id: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = self.pane_editor_mut(pane_id) {
            session.push_header_row("", "", true, true, window, cx);
        }
        cx.notify();
    }

    fn pane_remove_header(
        &mut self,
        pane_id: PaneId,
        row_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.pane_editor_mut(pane_id) {
            session.headers.retain(|row| row.id != row_id);
        }
        let _ = window;
        cx.notify();
    }

    fn pane_toggle_header(
        &mut self,
        pane_id: PaneId,
        row_id: usize,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.pane_editor_mut(pane_id)
            && let Some(row) = session.headers.iter_mut().find(|row| row.id == row_id)
        {
            row.enabled = enabled;
        }
        cx.notify();
    }

    fn pane_toggle_header_sharing(
        &mut self,
        pane_id: PaneId,
        row_id: usize,
        shared: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.pane_editor_mut(pane_id)
            && let Some(row) = session.headers.iter_mut().find(|row| row.id == row_id)
        {
            row.shared = shared;
        }
        cx.notify();
    }

    fn pane_select_body_mode(&mut self, pane_id: PaneId, mode: BodyMode, cx: &mut Context<Self>) {
        if let Some(session) = self.pane_editor_mut(pane_id) {
            session.body_mode = mode;
            if mode == BodyMode::Raw {
                session.refresh_raw_body_language(cx);
            }
        }
        cx.notify();
    }

    fn pane_select_raw_language(
        &mut self,
        pane_id: PaneId,
        language: RawBodyLanguage,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.pane_editor_mut(pane_id) {
            session.raw_body_language = language;
            session.refresh_raw_body_language(cx);
        }
        cx.notify();
    }

    fn pane_push_body_field(
        &mut self,
        pane_id: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.pane_editor_mut(pane_id) {
            session.push_body_field_row("", "", true, BodyFieldKind::Text, window, cx);
        }
        cx.notify();
    }

    fn pane_remove_body_field(
        &mut self,
        pane_id: PaneId,
        row_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.pane_editor_mut(pane_id) {
            session.body_fields.retain(|row| row.id != row_id);
        }
        let _ = window;
        cx.notify();
    }

    fn pane_toggle_body_field(
        &mut self,
        pane_id: PaneId,
        row_id: usize,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.pane_editor_mut(pane_id)
            && let Some(row) = session.body_fields.iter_mut().find(|row| row.id == row_id)
        {
            row.enabled = enabled;
        }
        cx.notify();
    }

    fn pane_set_body_field_kind(
        &mut self,
        pane_id: PaneId,
        row_id: usize,
        kind: BodyFieldKind,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.pane_editor_mut(pane_id)
            && let Some(row) = session.body_fields.iter_mut().find(|row| row.id == row_id)
        {
            row.kind = kind;
        }
        cx.notify();
    }

    fn pane_set_response_tab(&mut self, pane_id: PaneId, index: usize, cx: &mut Context<Self>) {
        if let Some(session) = self.pane_editor_mut(pane_id) {
            session.response_tab = ResponseTab::from_index(index);
            session.copied = false;
        }
        cx.notify();
    }

    fn pane_toggle_pretty(&mut self, pane_id: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let formatter = self.settings.formatter.clone();
        if let Some(session) = self.pane_editor_mut(pane_id) {
            session.pretty_body = !session.pretty_body;
            session.copied = false;
            session.formatted_body = session.response.as_ref().and_then(|response| {
                is_probably_text(&response.body).then(|| {
                    SharedString::from(format_body(&response.body, session.pretty_body, &formatter))
                })
            });
            if let Some(response) = &session.response {
                let language = response_language(response);
                let content = session.formatted_body.clone().map_or_else(
                    || format!("Binary response ({}).", format_bytes(response.size_bytes())),
                    |body| body.to_string(),
                );
                session.response_editor.update(cx, |editor, cx| {
                    editor.set_language(language, cx);
                    editor.set_value(content, window, cx);
                });
            }
        }
        cx.notify();
    }

    fn pane_copy_response(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        let formatter = self.settings.formatter.clone();
        let Some(session) = self.pane_editor_mut(pane_id) else {
            return;
        };
        let Some(response) = session.response.clone() else {
            return;
        };
        let value = match session.response_tab {
            ResponseTab::Headers => response
                .headers
                .iter()
                .map(|header| format!("{}: {}", header.name, header.value))
                .collect::<Vec<_>>()
                .join("\n"),
            ResponseTab::Preview | ResponseTab::Body => {
                format_body(&response.body, session.pretty_body, &formatter)
            }
            ResponseTab::Scripts => {
                session.copied = false;
                return;
            }
        };
        cx.write_to_clipboard(ClipboardItem::new_string(value));
        session.copied = true;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, px, size};
    use std::time::Duration;

    #[gpui::test]
    fn body_documentation_is_scoped_to_each_pane_session(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
        store.initialize().unwrap();
        let mut app = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let bindings = shortcuts::capture_base_key_bindings(cx);
            let view = cx.new(|cx| ApiTester::new_with_database_store(bindings, store, window, cx));
            app = Some(view.clone());
            Root::new(view, window, cx)
        });
        let app = app.unwrap();
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let primary_id = app.panes.panes()[0].id();
                let secondary_id = app
                    .panes
                    .split_off_pane(primary_id, SplitDirection::Vertical, true)
                    .expect("split primary pane");
                let tab_id = app.request_tabs.active_tab_id().clone();
                let session = PaneEditorState::new(secondary_id, tab_id, window, cx);
                let source = r#"{"name":"{{name}}"}"#;
                app.body
                    .update(cx, |editor, cx| editor.set_value(source, window, cx));
                session
                    .body
                    .update(cx, |editor, cx| editor.set_value(source, window, cx));
                app.documentation.update(cx, |editor, cx| {
                    editor.set_value("@body /name Primary name.", window, cx)
                });
                session.documentation.update(cx, |editor, cx| {
                    editor.set_value("@body /name Secondary name.", window, cx)
                });
                app.pane_editors.insert(secondary_id, session);
                app.render_request_panel(cx);
                app.render_pane_request_panel(secondary_id, cx);
                let offset = source.find("{{").unwrap();
                let session = app.pane_editors.get(&secondary_id).unwrap();
                assert_eq!(
                    documentation::tests::body_hover(&app.body, offset, window, cx).as_deref(),
                    Some("Primary name."),
                );
                assert_eq!(
                    documentation::tests::body_hover(&session.body, offset, window, cx).as_deref(),
                    Some("Secondary name."),
                );
                session.body.update(cx, |editor, cx| {
                    editor.set_value(r#"{"other":"value"}"#, window, cx)
                });
                assert!(
                    documentation::tests::body_hover(&session.body, offset, window, cx).is_none()
                );
                app.render_pane_request_panel(secondary_id, cx);
                let session = app.pane_editors.get(&secondary_id).unwrap();
                assert!(
                    session
                        .documentation_intelligence
                        .body_explanations("@body /name Secondary name.", offset)
                        .is_empty()
                );
                assert_eq!(
                    documentation::tests::body_hover(&app.body, offset, window, cx).as_deref(),
                    Some("Primary name."),
                );
            });
        });
    }

    fn template_with_content() -> RequestTemplate {
        RequestTemplate {
            request: RequestDraft {
                method: "POST".to_owned(),
                url: "https://example.com/submit".to_owned(),
                query_params: Vec::new(),
                headers: vec![
                    HeaderEntry {
                        enabled: true,
                        shared: true,
                        name: "Content-Type".to_owned(),
                        value: "application/json".to_owned(),
                    },
                    HeaderEntry {
                        enabled: true,
                        shared: true,
                        name: "X-Trace".to_owned(),
                        value: "abc".to_owned(),
                    },
                ],
                body: "{\"ok\": true}".to_owned(),
                body_mode: BodyMode::Raw,
                raw_body_language: RawBodyLanguage::Json,
                body_fields: Vec::new(),
            },
            scripts: RequestScripts {
                pre_request: "api.log('pre');".to_owned(),
                post_response: "api.log('post');".to_owned(),
            },
            documentation: "# Split pane notes\n\n@header X-Trace Secondary tracing explanation.".to_owned(),
            websocket: None,
        }
    }

    fn completed_response() -> ResponseData {
        ResponseData {
            status: 200,
            status_text: "OK".to_owned(),
            http_version: "HTTP/2".to_owned(),
            final_url: "https://example.com/submit".to_owned(),
            headers: Vec::new(),
            content_type: Some("application/json".to_owned()),
            body: br#"{"ok":true}"#.to_vec().into(),
            duration: Duration::from_millis(42),
        }
    }

    #[gpui::test]
    fn documentation_survives_http_and_websocket_tab_switches(cx: &mut TestAppContext) {
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

        let (http, websocket) = cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_blank_request_tab(window, cx);
                let http = app.request_tabs.active_tab_id().clone();
                app.documentation.update(cx, |editor, cx| {
                    editor.set_value("# HTTP notes", window, cx)
                });
                app.open_blank_websocket_tab(window, cx);
                let websocket = app.request_tabs.active_tab_id().clone();
                assert!(app.documentation.read(cx).value(cx).is_empty());
                app.documentation.update(cx, |editor, cx| {
                    editor.set_value("# Socket notes", window, cx)
                });
                (http, websocket)
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.activate_request_tab(http, window, cx);
                assert_eq!(
                    app.documentation.read(cx).value(cx).as_ref(),
                    "# HTTP notes"
                );
                app.request_pane = RequestPane::Documentation;
                app.render_request_panel(cx);
                app.activate_request_tab(websocket, window, cx);
                assert_eq!(
                    app.documentation.read(cx).value(cx).as_ref(),
                    "# Socket notes"
                );
                assert_eq!(app.request_template(cx).documentation, "# Socket notes");
            });
        });
    }

    #[gpui::test]
    fn secondary_pane_gets_a_real_request_editor_session(cx: &mut TestAppContext) {
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
                app.open_blank_request_tab(window, cx);
                app.open_blank_request_tab(window, cx);
            });
        });
        cx.run_until_parked();

        let (first, second, third) = cx.update(|_, cx| {
            let app = app.read(cx);
            let ids = app
                .request_tabs
                .tabs()
                .iter()
                .map(|tab| tab.id().clone())
                .collect::<Vec<_>>();
            (ids[0].clone(), ids[1].clone(), ids[2].clone())
        });

        // Give the second tab a known template so the round-trip is meaningful,
        // and pin the pane tree to the two open tabs with the first tab active
        // so the freshly created pane is the genuine secondary pane.
        let tmpl = template_with_content();
        let (_primary_id, secondary_id) = cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let _ = app.request_tabs.activate(&first);
                app.request_tabs
                    .get_mut(&second)
                    .expect("second tab")
                    .set_template(tmpl.clone());
                app.request_tabs
                    .get_mut(&third)
                    .expect("third tab")
                    .set_template(tmpl.clone());
                app.request_tab_runtime.insert(
                    second.as_str().to_owned(),
                    RequestTabRuntime {
                        response: Some(completed_response()),
                        ..RequestTabRuntime::default()
                    },
                );
                app.panes = PaneRoot::from_tabs(
                    vec![
                        WorkspaceTab::Request(first.clone()),
                        WorkspaceTab::Request(second.clone()),
                        WorkspaceTab::Request(third.clone()),
                    ],
                    0,
                );
                let primary_id = app.panes.panes()[0].id();
                let secondary_id = app
                    .panes
                    .split_off_pane(primary_id, SplitDirection::Vertical, true)
                    .expect("split the primary pane");
                app.panes.move_tab_between_panes(
                    &WorkspaceTab::Request(second.clone()),
                    primary_id,
                    secondary_id,
                    0,
                );
                app.panes.move_tab_between_panes(
                    &WorkspaceTab::Request(third.clone()),
                    primary_id,
                    secondary_id,
                    1,
                );
                app.reconcile_pane_editors(window, cx);
                (primary_id, secondary_id)
            })
        });
        cx.run_until_parked();

        assert!(
            cx.debug_bounds("secondary-pane-send-request").is_some(),
            "a secondary request pane must retain its Send button",
        );
        assert!(
            cx.debug_bounds("secondary-pane-response-code-editor")
                .is_some(),
            "secondary responses must render through the same code-editor surface",
        );

        cx.update(|_, cx| {
            let app = app.read(cx);
            assert!(
                app.primary_pane_scroll.max_offset().height > px(0.),
                "a short primary pane must expose vertical overflow",
            );
            assert!(
                app.pane_editors
                    .get(&secondary_id)
                    .expect("secondary pane session")
                    .pane_scroll
                    .max_offset()
                    .height
                    > px(0.),
                "a short secondary pane must expose vertical overflow",
            );
            assert_eq!(
                app.pane_editors
                    .get(&secondary_id)
                    .and_then(|session| session.response.as_ref())
                    .map(|response| response.status),
                Some(200),
                "the secondary pane must render its stored response",
            );
        });

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.activate_workspace_tab_in_pane(
                    WorkspaceTab::Request(third.clone()),
                    Some(secondary_id),
                    window,
                    cx,
                );
            });
        });
        cx.run_until_parked();

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.activate_workspace_tab_in_pane(
                    WorkspaceTab::Request(second.clone()),
                    Some(secondary_id),
                    window,
                    cx,
                );
            });
        });
        cx.run_until_parked();

        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                assert_eq!(
                    app.request_tabs.active_tab_id(),
                    &first,
                    "secondary-pane selection must not replace the global request surface",
                );
                assert_eq!(
                    app.panes
                        .pane(secondary_id)
                        .and_then(|pane| pane.active_tab()),
                    Some(WorkspaceTab::Request(second.clone())),
                );
                assert!(app.panes.pane(secondary_id).is_some());
                let session = app
                    .pane_editors
                    .get(&secondary_id)
                    .expect("secondary pane must own a request editor session");
                assert_eq!(
                    documentation::explanation(
                        &session.documentation_intelligence,
                        &session.documentation,
                        crate::documentation_intelligence::TargetKind::Header,
                        "x-trace",
                        cx,
                    ).as_deref(),
                    Some("Secondary tracing explanation."),
                );
                assert_eq!(
                    documentation::explanation(
                        &app.documentation_intelligence,
                        &app.documentation,
                        crate::documentation_intelligence::TargetKind::Header,
                        "x-trace",
                        cx,
                    ),
                    None,
                    "secondary explanations must not leak into the primary request",
                );
                assert!(
                    session.documentation_intelligence.diagnostics(
                        session.documentation.read(cx).value(cx).as_ref(),
                    ).is_empty(),
                    "secondary diagnostics must resolve against secondary header names",
                );
                assert_eq!(session.active_tab_id.as_ref(), Some(&second));
                assert_eq!(
                    session.response.as_ref().map(|response| response.status),
                    Some(200),
                    "switching away and back must restore the pane-local response",
                );
                assert_eq!(
                    session.method.read(cx).value().as_ref(),
                    "POST",
                    "the session must restore the moved tab's method"
                );

                let round_trip = session.snapshot_template(cx);
                assert_eq!(round_trip.request, tmpl.request);
                assert_eq!(round_trip.scripts, tmpl.scripts);
            });
        });

        // Completions are keyed to request tabs, not to the globally active
        // request or the pane's current selection. Finishing one request must
        // leave another request running and its response must survive a
        // switch away and back.
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.pane_requests_in_flight
                    .insert(second.as_str().to_owned(), 41);
                app.pane_requests_in_flight
                    .insert(third.as_str().to_owned(), 42);
                let resolved = resolve_request(&tmpl.request, None).expect("resolve request");
                app.finish_pane_request(
                    second.clone(),
                    41,
                    resolved,
                    Ok(completed_response()),
                    window,
                    cx,
                );
                assert!(!app.pane_requests_in_flight.contains_key(second.as_str()));
                assert_eq!(
                    app.pane_requests_in_flight.get(third.as_str()),
                    Some(&42),
                    "a different pane request must remain in flight",
                );
            });
        });
        cx.run_until_parked();

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.activate_workspace_tab_in_pane(
                    WorkspaceTab::Request(third.clone()),
                    Some(secondary_id),
                    window,
                    cx,
                );
                app.activate_workspace_tab_in_pane(
                    WorkspaceTab::Request(second.clone()),
                    Some(secondary_id),
                    window,
                    cx,
                );
                assert_eq!(
                    app.pane_editors
                        .get(&secondary_id)
                        .and_then(|session| session.response.as_ref())
                        .map(|response| response.status),
                    Some(200),
                    "completed response must remain attached to its request tab",
                );
            });
        });
    }
}
