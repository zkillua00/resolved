use super::*;

impl ApiTester {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let script_variable_catalog = ScriptVariableCatalog::default().shared();
        let template_variable_catalog = TemplateVariableCatalog::default().shared();
        let method = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("METHOD")
                .default_value("GET")
        });
        let url_template_catalog = Rc::clone(&template_variable_catalog);
        let url = cx.new(|cx| {
            template_input_state(
                window,
                cx,
                url_template_catalog,
                "https://api.example.com/users",
                "https://httpbin.org/get",
            )
        });
        let body_completion_catalog = Rc::clone(&template_variable_catalog);
        let body_hover_catalog = Rc::clone(&template_variable_catalog);
        let body = cx.new(|cx| {
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(CodeLanguage::Json)
                    .placeholder("Raw request body · ⌘F to search")
                    .rows(12)
                    .soft_wrap(false)
                    .format_action(true)
                    .completion_provider(Rc::new(TemplateCompletionProvider::new(
                        body_completion_catalog,
                    )))
                    .hover_provider(Rc::new(TemplateHoverProvider::new(body_hover_catalog))),
                window,
                cx,
            )
        });
        let pre_completion_catalog = Rc::clone(&script_variable_catalog);
        let pre_diagnostic_catalog = Rc::clone(&script_variable_catalog);
        let pre_request_script = cx.new(|cx| {
            let completion_catalog = Rc::clone(&pre_completion_catalog);
            let diagnostic_catalog = Rc::clone(&pre_diagnostic_catalog);
            let intelligence = Rc::new(ScriptCompletionProvider::new(
                ScriptEditorPhase::PreRequest,
                completion_catalog,
            ));
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(CodeLanguage::JavaScript)
                    .placeholder(
                        "api.request.headers.set(\"X-Token\", api.environment.get(\"token\"));",
                    )
                    .rows(12)
                    .soft_wrap(false)
                    .completion_provider(intelligence.clone())
                    .hover_provider(intelligence)
                    .diagnostic_provider(move |source| {
                        diagnostics_for_source(source, &diagnostic_catalog.borrow())
                            .into_iter()
                            .map(Into::into)
                            .collect()
                    }),
                window,
                cx,
            )
        });
        let post_completion_catalog = Rc::clone(&script_variable_catalog);
        let post_diagnostic_catalog = Rc::clone(&script_variable_catalog);
        let post_response_script = cx.new(|cx| {
            let completion_catalog = Rc::clone(&post_completion_catalog);
            let diagnostic_catalog = Rc::clone(&post_diagnostic_catalog);
            let intelligence = Rc::new(ScriptCompletionProvider::new(
                ScriptEditorPhase::PostResponse,
                completion_catalog,
            ));
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(CodeLanguage::JavaScript)
                    .placeholder(
                        "api.test(\"status is 200\", () => api.assert(api.response.status === 200));",
                    )
                    .rows(12)
                    .soft_wrap(false)
                    .completion_provider(intelligence.clone())
                    .hover_provider(intelligence)
                    .diagnostic_provider(move |source| {
                        diagnostics_for_source(source, &diagnostic_catalog.borrow())
                            .into_iter()
                            .map(Into::into)
                            .collect()
                    }),
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
        let debug_overlay = cx.new(DebugOverlay::new);

        let database_store = DatabaseStore::default();
        let (
            history,
            workspace,
            history_warning,
            workspace_warning,
            history_writable,
            workspace_writable,
        ) = match database_store.initialize() {
            Ok(()) => {
                let import_warning = database_store
                    .import_legacy_if_needed()
                    .err()
                    .map(|error| format!("Legacy JSON data could not be imported: {error}"));
                let (history, history_warning, history_writable) =
                    match database_store.load_history() {
                        Ok(history) => (history, None, true),
                        Err(error) => (
                            RequestHistory::default(),
                            Some(format!(
                                "History could not be loaded and will not be overwritten: {error}"
                            )),
                            false,
                        ),
                    };
                let (workspace, workspace_load_warning, workspace_writable) =
                    match database_store.load_workspace() {
                        Ok(workspace) => (workspace, None, true),
                        Err(error) => (
                            Workspace::default(),
                            Some(format!(
                                "Workspace could not be loaded and will not be overwritten: {error}"
                            )),
                            false,
                        ),
                    };
                let workspace_warning = match (workspace_load_warning, import_warning) {
                    (Some(load), Some(import)) => Some(format!("{load}\n{import}")),
                    (Some(load), None) => Some(load),
                    (None, Some(import)) => Some(import),
                    (None, None) => None,
                };
                (
                    history,
                    workspace,
                    history_warning,
                    workspace_warning,
                    history_writable,
                    workspace_writable,
                )
            }
            Err(error) => (
                RequestHistory::default(),
                Workspace::default(),
                Some(format!(
                    "Database could not be opened and history will not be overwritten: {error}"
                )),
                Some(format!(
                    "Database could not be opened and workspace will not be overwritten: {error}"
                )),
                false,
                false,
            ),
        };
        let selected_collection_id = workspace
            .collections
            .first()
            .map(|collection| collection.id.clone());
        update_script_variable_catalog(&script_variable_catalog, &workspace);
        template_variable_catalog
            .borrow_mut()
            .replace_environment(workspace.active_environment());
        let selected_environment_id = workspace.active_environment_id.clone().or_else(|| {
            workspace
                .environments
                .first()
                .map(|environment| environment.id.clone())
        });
        let selected_collection_name = selected_collection_id
            .as_deref()
            .and_then(|id| workspace.collection(id))
            .map(|collection| collection.name.clone())
            .unwrap_or_default();
        let collection_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search collections"));
        let environment_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search environments"));
        let expanded_collection_ids = workspace
            .collections
            .iter()
            .map(|collection| collection.id.clone())
            .collect();
        let selected_environment_name = selected_environment_id
            .as_deref()
            .and_then(|id| workspace.environment(id))
            .map(|environment| environment.name.clone())
            .unwrap_or_default();
        let collection_name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Collection name")
                .default_value(selected_collection_name)
        });
        let collection_delete_confirmation =
            cx.new(|cx| InputState::new(window, cx).placeholder("Type the collection name"));
        let saved_request_name =
            cx.new(|cx| InputState::new(window, cx).placeholder("Request name"));
        let environment_name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Environment name")
                .default_value(selected_environment_name)
        });
        let environment_variables = Self::environment_rows(
            selected_environment_id
                .as_deref()
                .and_then(|id| workspace.environment(id)),
            window,
            cx,
        );

        let client = build_client().expect("failed to create the HTTP client");
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("api-tester-network")
                .enable_all()
                .build()
                .expect("failed to create the network runtime"),
        );

        let url_subscription = cx.subscribe_in(&url, window, |this, input, event, window, cx| {
            this.track_template_input_focus(input, event);
            if matches!(event, InputEvent::Change) {
                let input = this.url.clone();
                this.schedule_template_input_refresh(&input, cx);
                this.refresh_request_dirty_part(RequestDirtyPart::Url, cx);
            }
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.start_request(window, cx);
            }
        });
        let method_subscription = cx.subscribe_in(&method, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                this.refresh_request_dirty_part(RequestDirtyPart::Method, cx);
            }
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.start_request(window, cx);
            }
        });
        let body_subscription = cx.subscribe(&body, |this, editor, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let input = editor.read(cx).input_state();
                this.schedule_template_input_refresh(&input, cx);
                this.refresh_request_dirty_part(RequestDirtyPart::RawBody, cx);
            }
        });
        let body_format_subscription =
            cx.subscribe_in(&body, window, |this, _, event, window, cx| {
                if matches!(event, CodeEditorEvent::FormatRequested) {
                    this.format_raw_body(window, cx);
                }
            });
        let pre_request_subscription =
            cx.subscribe(&pre_request_script, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.refresh_request_dirty_part(RequestDirtyPart::PreScript, cx);
                }
            });
        let post_response_subscription =
            cx.subscribe(&post_response_script, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.refresh_request_dirty_part(RequestDirtyPart::PostScript, cx);
                }
            });
        let collection_name_subscription =
            cx.subscribe_in(&collection_name, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.rename_collection(cx);
                    this.renaming_collection_id = None;
                    cx.notify();
                }
            });

        let mut this = Self {
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
            request_tab: RequestTab::Headers,
            response_tab: ResponseTab::Body,
            pretty_body: true,
            sending: false,
            execution_stage: None,
            request_generation: 0,
            abort_handle: None,
            script_cancellation: None,
            response: None,
            request_error: None,
            script_diagnostic: None,
            pre_script_report: None,
            post_script_report: None,
            preview_error: None,
            copied: false,
            client,
            runtime,
            history,
            history_warning,
            history_writable,
            workspace,
            database_store,
            workspace_warning,
            workspace_writable,
            sidebar_tab: SidebarTab::Collections,
            navigation_compact: false,
            selected_collection_id,
            active_saved_request_id: None,
            detached_request_dirty: false,
            request_dirty: RequestDirtyState::default(),
            loaded_request_baseline: RequestTemplate::default(),
            pending_request_load_key: None,
            request_notice: None,
            selected_environment_id,
            collection_search,
            environment_search,
            expanded_collection_ids,
            renaming_collection_id: None,
            collection_name,
            collection_delete_confirmation,
            saved_request_name,
            environment_name,
            environment_variables,
            next_variable_row_id: 0,
            pending_delete: None,
            script_variable_catalog,
            template_variable_catalog,
            template_highlight_tasks: HashMap::new(),
            template_variable_popover: None,
            focused_template_input: None,
            debug_overlay,
            preview: None,
            _subscriptions: vec![
                url_subscription,
                method_subscription,
                body_subscription,
                body_format_subscription,
                pre_request_subscription,
                post_response_subscription,
                collection_name_subscription,
            ],
        };
        this.push_header_row("", "", true, window, cx);
        this.refresh_variable_intelligence(cx);
        this.loaded_request_baseline = this.request_template(cx);
        this.request_dirty.clear();
        this
    }
}
