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
    pre_request_script: Entity<CodeEditor>,
    post_response_script: Entity<CodeEditor>,
    response_editor: Entity<CodeEditor>,
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
    request_error: Option<String>,
    script_diagnostic: Option<ScriptDiagnostic>,
    pre_script_report: Option<ScriptReport>,
    post_script_report: Option<ScriptReport>,
    preview_error: Option<String>,
    copied: bool,
    request_notice: Option<String>,
}

impl PaneEditorState {
    fn new(
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
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("URL"));
        let body = cx.new(|cx| {
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(CodeLanguage::Json)
                    .placeholder("Raw request body")
                    .rows(12)
                    .soft_wrap(false)
                    .format_action(true)
                    .context_menu_builder(snippet_context_menu_builder(
                        snippet_menu_owner.clone(),
                        SnippetMenuSurface::RequestBody,
                    )),
                window,
                cx,
            )
        });
        let pre_request_script = cx.new(|cx| {
            CodeEditor::new(
                CodeEditorConfig::default()
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
            pre_request_script,
            post_response_script,
            response_editor,
            headers: Vec::new(),
            next_header_id: 0,
            body_mode: BodyMode::Raw,
            raw_body_language: RawBodyLanguage::Json,
            body_fields: Vec::new(),
            next_body_field_id: 0,
            request_pane: RequestPane::Headers,
            response_tab: ResponseTab::Body,
            pretty_body: true,
            response: None,
            response_request: None,
            response_sensitive_values: Vec::new(),
            request_error: None,
            script_diagnostic: None,
            pre_script_report: None,
            post_script_report: None,
            preview_error: None,
            copied: false,
            request_notice: None,
        };
        this.set_raw_body_language(cx);
        this
    }

    fn set_raw_body_language(&mut self, cx: &mut Context<ApiTester>) {
        let language = code_language_for_raw_body(self.raw_body_language);
        self.body.update(cx, |editor, cx| editor.set_language(language, cx));
    }

    /// Refresh the raw body editor's highlight to match `raw_body_language`.
    fn refresh_raw_body_language(&mut self, cx: &mut Context<ApiTester>) {
        let language = code_language_for_raw_body(self.raw_body_language);
        self.body.update(cx, |editor, cx| editor.set_language(language, cx));
    }

    fn push_header_row(
        &mut self,
        name: impl Into<SharedString>,
        value: impl Into<SharedString>,
        enabled: bool,
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
        template: &RequestTemplate,
        runtime: &RequestTabRuntime,
        window: &mut Window,
        cx: &mut Context<ApiTester>,
    ) {
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
                window,
                cx,
            );
        }
        if self.headers.is_empty() {
            self.push_header_row("", "", true, window, cx);
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
        self.response_request = runtime.response_request.clone();
        self.response_sensitive_values = runtime.response_sensitive_values.clone();
        self.request_error = runtime.request_error.clone();
        self.script_diagnostic = runtime.script_diagnostic.clone();
        self.pre_script_report = runtime.pre_script_report.clone();
        self.post_script_report = runtime.post_script_report.clone();
        self.preview_error = runtime.preview_error.clone();
        self.copied = runtime.copied;
        self.request_notice = runtime.request_notice.clone();
        let content = match &self.response {
            Some(response) if is_probably_text(&response.body) => None,
            Some(response) => Some(format!(
                "Binary response ({}).",
                format_bytes(response.size_bytes())
            )),
            None => None,
        };
        if let Some(content) = content {
            self.response_editor.update(cx, |editor, cx| {
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
        RequestTemplate {
            request: RequestDraft {
                method: self.method.read(cx).value().to_string(),
                url: self.url.read(cx).value().to_string(),
                headers: self
                    .headers
                    .iter()
                    .map(|row| HeaderEntry {
                        enabled: row.enabled,
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
        }
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

    /// Build (or refresh) the request-editor session for `pane_id` so it tracks
    /// the pane's active request tab, and drop sessions for panes that no
    /// longer exist.
    pub(super) fn reconcile_pane_editors(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active_workspace_tab = self.workspace_tabs.active_tab(&self.request_tabs);
        let primary_pane_id = self.panes.pane_for_tab(&active_workspace_tab);
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
            let active_tab = self
                .panes
                .pane(pane_id)
                .and_then(|pane| pane.active_tab());
            let Some(WorkspaceTab::Request(tab_id)) = active_tab else {
                continue;
            };
            self.ensure_pane_editor_for(pane_id, tab_id, window, cx);
        }
        self.pane_editors.retain(|pane_id, _| live.contains(pane_id));
    }

    fn ensure_pane_editor_for(
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
                self.request_tab_runtime.insert(old_id.as_str().to_owned(), runtime);
            }
            let mut session = PaneEditorState::new(tab_id.clone(), window, cx);
            session.load_template(record.template(), &runtime, window, cx);
            session.active_tab_id = Some(tab_id);
            self.pane_editors.insert(pane_id, session);
        }
    }
}

impl PaneEditorState {
    fn dom_key(&self, pane_id: PaneId) -> SharedString {
        format!("pane-{}", pane_id.0).into()
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
            .can_drop(move |value, _, _| {
                value.downcast_ref::<WorkspaceTabDrag>().is_some()
            })
            .drag_over::<WorkspaceTabDrag>(move |style, _, _, cx| {
                style.bg(cx.theme().drop_target.opacity(0.35))
            })
            .on_drop(cx.listener(move |this, drag: &WorkspaceTabDrag, window, cx| {
                this.on_workspace_tab_move(drag, pane_id, insert_index, window, cx);
            }))
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
        let header_count = session
            .headers
            .iter()
            .filter(|row| row.enabled && !input_text_is_blank(&row.name, cx))
            .count();
        let key = session.dom_key(pane_id);

        v_flex()
            .size_full()
            .min_h_0()
            .gap_3()
            .p_4()
            .bg(cx.api_surface())
            .child(self.render_pane_url_row(session, pane_id, cx))
            .child(
                TabBar::new(SharedString::from(format!("{key}-request-tabs")))
                    .underline()
                    .children([
                        format!("Headers ({header_count})"),
                        "Body".to_owned(),
                        "Pre-request".to_owned(),
                        "Post-response".to_owned(),
                    ])
                    .selected_index(session.request_pane.index())
                    .on_click(cx.listener(move |this, index: &usize, _, cx| {
                        this.pane_set_request_pane(pane_id, RequestPane::from_index(*index), cx);
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .when(session.request_pane == RequestPane::Headers, |this| {
                        this.child(self.render_pane_headers_editor(session, pane_id, cx))
                    })
                    .when(session.request_pane == RequestPane::Body, |this| {
                        this.child(self.render_pane_body_editor(session, pane_id, cx))
                    })
                    .when(session.request_pane == RequestPane::PreRequest, |this| {
                        this.child(session.pre_request_script.clone())
                    })
                    .when(session.request_pane == RequestPane::PostResponse, |this| {
                        this.child(session.post_response_script.clone())
                    }),
            )
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
            .into_any_element()
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

        v_flex()
            .size_full()
            .min_h_0()
            .rounded_lg()
            .border_1()
            .border_color(cx.api_outline_variant())
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
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .child(format!("{enabled_count} enabled")),
                    ),
            )
            .child(
                h_flex()
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
                    .child(div().w(px(44.))),
            )
            .child(
                v_flex()
                    .id(SharedString::from(format!("{key}-headers-scroll")))
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
                            .child(
                                Button::new("add-pane-header")
                                    .icon(IconName::Plus)
                                    .label("Add header")
                                    .small()
                                    .ghost()
                                    .on_click(
                                        cx.listener(move |this, _, window, cx| {
                                            this.pane_push_header(pane_id, window, cx);
                                        }),
                                    ),
                            ),
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
        h_flex()
            .id(SharedString::from(format!("{key}-header-row-{id}")))
            .w_full()
            .h(px(44.))
            .flex_shrink_0()
            .border_t_1()
            .border_color(cx.api_outline_variant())
            .bg(cx.api_surface())
            .when(!row.enabled, |this| this.opacity(0.55))
            .child(
                div()
                    .w(px(44.))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Checkbox::new(SharedString::from(format!("{key}-header-enabled-{id}")))
                            .checked(row.enabled)
                            .small()
                            .on_click(cx.listener(
                                move |this, checked: &bool, _, cx| {
                                    this.pane_toggle_header(pane_id, id, *checked, cx);
                                },
                            )),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .border_l_1()
                    .border_color(cx.api_outline_variant())
                    .child(
                        Input::new(&row.name)
                            .appearance(false)
                            .small()
                            .size_full()
                            .px_3(),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .border_l_1()
                    .border_color(cx.api_outline_variant())
                    .child(
                        Input::new(&row.value)
                            .appearance(false)
                            .small()
                            .size_full()
                            .px_3(),
                    ),
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
                        Button::new(SharedString::from(format!("{key}-header-delete-{id}")))
                            .icon(IconName::Delete)
                            .xsmall()
                            .ghost()
                            .tooltip("Delete header")
                            .on_click(
                                cx.listener(move |this, _, window, cx| {
                                    this.pane_remove_header(pane_id, id, window, cx);
                                }),
                            ),
                    ),
            )
            .into_any_element()
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
            BodyMode::Raw => div().size_full().child(session.body.clone()).into_any_element(),
            BodyMode::FormUrlEncoded => {
                self.render_pane_body_fields(session, pane_id, false, cx)
            }
            BodyMode::MultipartFormData => {
                self.render_pane_body_fields(session, pane_id, true, cx)
            }
        };

        v_flex()
            .size_full()
            .min_h_0()
            .gap_3()
            .child(self.render_pane_body_mode_toolbar(session, pane_id, cx))
            .child(div().flex_1().min_h_0().child(content))
            .into_any_element()
    }

    fn render_pane_body_mode_toolbar(
        &self,
        session: &PaneEditorState,
        pane_id: PaneId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mode_buttons = BodyMode::all()
            .iter()
            .copied()
            .enumerate()
            .map(|(index, mode)| {
                Button::new(("pane-body-mode", index))
                    .label(mode.label())
                    .small()
                    .ghost()
                    .rounded(px(18.))
                    .selected(session.body_mode == mode)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pane_select_body_mode(pane_id, mode, cx);
                    }))
            })
            .collect::<Vec<_>>();
        let key = session.dom_key(pane_id);

        let selected_language = session.raw_body_language;
        let owner = cx.entity().downgrade();
        h_flex()
            .w_full()
            .flex_wrap()
            .justify_between()
            .gap_2()
            .child(h_flex().flex_wrap().gap_1().children(mode_buttons))
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

        v_flex()
            .size_full()
            .min_h_0()
            .rounded_lg()
            .border_1()
            .border_color(cx.api_outline_variant())
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
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .child(if multipart {
                                "form-data"
                            } else {
                                "x-www-form-urlencoded"
                            }),
                    ),
            )
            .child(
                v_flex()
                    .id(SharedString::from(format!("{key}-body-fields-scroll")))
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
                            .child(
                                Button::new("add-pane-body-field")
                                    .icon(IconName::Plus)
                                    .label("Add field")
                                    .small()
                                    .ghost()
                                    .on_click(
                                        cx.listener(move |this, _, window, cx| {
                                            this.pane_push_body_field(pane_id, window, cx);
                                        }),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
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

        h_flex()
            .id(SharedString::from(format!("{key}-body-field-row-{id}")))
            .w_full()
            .h(px(46.))
            .flex_shrink_0()
            .border_t_1()
            .border_color(cx.api_outline_variant())
            .bg(cx.api_surface())
            .when(!row.enabled, |this| this.opacity(0.55))
            .child(
                div()
                    .w(px(44.))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Checkbox::new(SharedString::from(format!("{key}-body-field-enabled-{id}")))
                            .checked(row.enabled)
                            .small()
                            .on_click(cx.listener(
                                move |this, checked: &bool, _, cx| {
                                    this.pane_toggle_body_field(pane_id, id, *checked, cx);
                                },
                            )),
                    ),
            )
            .when(multipart, |this| {
                this.child(
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
                        ),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .border_l_1()
                    .border_color(cx.api_outline_variant())
                    .child(
                        Input::new(&row.name)
                            .appearance(false)
                            .small()
                            .size_full()
                            .px_3(),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .border_l_1()
                    .border_color(cx.api_outline_variant())
                    .child(
                        Input::new(&row.value)
                            .appearance(false)
                            .small()
                            .size_full()
                            .px_3(),
                    ),
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
                        Button::new(SharedString::from(format!("{key}-body-field-delete-{id}")))
                            .icon(IconName::Delete)
                            .xsmall()
                            .ghost()
                            .tooltip("Delete field")
                            .on_click(
                                cx.listener(move |this, _, window, cx| {
                                    this.pane_remove_body_field(pane_id, id, window, cx);
                                }),
                            ),
                    ),
            )
            .into_any_element()
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
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.pane_toggle_pretty(pane_id, cx);
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
                    .p_4()
                    .when(session.response_tab == ResponseTab::Body, |this| {
                        this.child(self.render_pane_response_body(session, pane_id, cx))
                    })
                    .when(session.response_tab == ResponseTab::Headers, |this| {
                        this.child(self.render_pane_response_headers(session, pane_id, cx))
                    })
                    .when(
                        session.response_tab == ResponseTab::Preview,
                        |this| {
                            this.child(
                                v_flex()
                                    .size_full()
                                    .items_center()
                                    .justify_center()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(div().text_sm().child(
                                        "Live preview is not available in a secondary pane.",
                                    )),
                            )
                        },
                    )
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
        let Some(response) = &session.response else {
            return div().size_full().into_any_element();
        };
        let content = if is_probably_text(&response.body) {
            format_body(&response.body, session.pretty_body, &self.settings.formatter)
        } else {
            format!(
                "Binary response ({}).",
                format_bytes(response.size_bytes())
            )
        };
        div()
            .id(SharedString::from(format!("{key}-response-body")))
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .whitespace_nowrap()
            .font_family(cx.theme().mono_font_family.clone())
            .text_xs()
            .text_color(cx.theme().foreground)
            .child(content)
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

fn status_color(status: u16, cx: &App) -> Hsla {
    match status {
        200..=299 => cx.theme().success,
        400..=499 => cx.theme().warning,
        _ => cx.theme().danger,
    }
}

impl ApiTester {
    fn pane_set_request_pane(&mut self, pane_id: PaneId, pane: RequestPane, cx: &mut Context<Self>) {
        if let Some(session) = self.pane_editor_mut(pane_id) {
            session.request_pane = pane;
        }
        cx.notify();
    }

    fn pane_push_header(
        &mut self,
        pane_id: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.pane_editor_mut(pane_id) {
            session.push_header_row("", "", true, window, cx);
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

    fn pane_select_body_mode(
        &mut self,
        pane_id: PaneId,
        mode: BodyMode,
        cx: &mut Context<Self>,
    ) {
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

    fn pane_toggle_pretty(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        if let Some(session) = self.pane_editor_mut(pane_id) {
            session.pretty_body = !session.pretty_body;
            session.copied = false;
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
    use gpui::{TestAppContext, size, px};

    fn template_with_content() -> RequestTemplate {
        RequestTemplate {
            request: RequestDraft {
                method: "POST".to_owned(),
                url: "https://example.com/submit".to_owned(),
                headers: vec![
                    HeaderEntry {
                        enabled: true,
                        name: "Content-Type".to_owned(),
                        value: "application/json".to_owned(),
                    },
                    HeaderEntry {
                        enabled: true,
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
        }
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

        let (first, second) = cx.update(|_, cx| {
            let app = app.read(cx);
            let ids = app
                .request_tabs
                .tabs()
                .iter()
                .map(|tab| tab.id().clone())
                .collect::<Vec<_>>();
            (ids[0].clone(), ids[1].clone())
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
                app.panes = PaneRoot::from_tabs(
                    vec![
                        WorkspaceTab::Request(first.clone()),
                        WorkspaceTab::Request(second.clone()),
                    ],
                    0,
                );
                let primary_id = app.panes.panes()[0].id();
                let secondary_id = app
                    .panes
                    .split_off_pane(primary_id, SplitDirection::Vertical, true)
                    .expect("split the primary pane");
                app.panes
                    .move_tab_between_panes(
                        &WorkspaceTab::Request(second.clone()),
                        primary_id,
                        secondary_id,
                        0,
                    );
                app.reconcile_pane_editors(window, cx);
                (primary_id, secondary_id)
            })
        });
        cx.run_until_parked();

        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                assert!(app.panes.pane(secondary_id).is_some());
                let session = app
                    .pane_editors
                    .get(&secondary_id)
                    .expect("secondary pane must own a request editor session");
                assert_eq!(session.active_tab_id.as_ref(), Some(&second));
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
    }
}

