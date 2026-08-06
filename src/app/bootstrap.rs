use super::request_tab_reconciliation::reconcile_restored_request_tabs;
use super::*;

fn embedded_typescript_service() -> Option<TypeScriptServiceHandle> {
    static SERVICE: OnceLock<Option<TypeScriptServiceHandle>> = OnceLock::new();

    SERVICE
        .get_or_init(|| match TypeScriptServiceHandle::start() {
            Ok(service) => Some(service),
            Err(error) => {
                tracing::warn!(%error, "JavaScript language service unavailable");
                None
            }
        })
        .clone()
}

impl ApiTester {
    pub fn new(
        base_key_bindings: Vec<gpui::KeyBinding>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_database_store(base_key_bindings, DatabaseStore::default(), window, cx)
    }

    pub(super) fn new_with_database_store(
        base_key_bindings: Vec<gpui::KeyBinding>,
        database_store: DatabaseStore,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let snippet_menu_owner = cx.entity().downgrade();
        let script_variable_catalog = ScriptVariableCatalog::default().shared();
        let typescript_service = embedded_typescript_service();
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
        let body = cx.new(|cx| {
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(CodeLanguage::Json)
                    .placeholder("Raw request body · ⌘F to search")
                    .rows(12)
                    .soft_wrap(false)
                    .format_action(true)
                    .context_menu_builder(snippet_context_menu_builder(
                        snippet_menu_owner.clone(),
                        SnippetMenuSurface::RequestBody,
                    ))
                    .completion_provider(Rc::new(TemplateCompletionProvider::new(
                        body_completion_catalog,
                    ))),
                window,
                cx,
            )
        });
        let pre_completion_catalog = Rc::clone(&script_variable_catalog);
        let pre_request_script = cx.new(|cx| {
            let completion_catalog = Rc::clone(&pre_completion_catalog);
            let mut intelligence =
                ScriptCompletionProvider::new(ScriptEditorPhase::PreRequest, completion_catalog);
            if let Some(service) = typescript_service.clone() {
                intelligence = intelligence.with_typescript_service(service);
            }
            let intelligence = Rc::new(intelligence);
            let diagnostic_intelligence = Rc::clone(&intelligence);
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(CodeLanguage::JavaScript)
                    .placeholder(
                        "api.request.headers.set(\"X-Token\", api.environment.get(\"token\"));",
                    )
                    .rows(12)
                    .soft_wrap(false)
                    .format_action(true)
                    .context_menu_builder(snippet_context_menu_builder(
                        snippet_menu_owner.clone(),
                        SnippetMenuSurface::PreRequestScript,
                    ))
                    .completion_provider(intelligence.clone())
                    .hover_provider(intelligence)
                    .async_diagnostic_provider(move |source, cx| {
                        let diagnostics = diagnostic_intelligence.diagnostics_task(source, cx);
                        cx.background_spawn(async move {
                            diagnostics.await.into_iter().map(Into::into).collect()
                        })
                    }),
                window,
                cx,
            )
        });
        let post_completion_catalog = Rc::clone(&script_variable_catalog);
        let post_response_script = cx.new(|cx| {
            let completion_catalog = Rc::clone(&post_completion_catalog);
            let mut intelligence = ScriptCompletionProvider::new(
                ScriptEditorPhase::PostResponse,
                completion_catalog,
            );
            if let Some(service) = typescript_service.clone() {
                intelligence = intelligence.with_typescript_service(service);
            }
            let intelligence = Rc::new(intelligence);
            let diagnostic_intelligence = Rc::clone(&intelligence);
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(CodeLanguage::JavaScript)
                    .placeholder(
                        "api.test(\"status is 200\", () => api.assert(api.response.status === 200));",
                    )
                    .rows(12)
                    .soft_wrap(false)
                    .format_action(true)
                    .context_menu_builder(snippet_context_menu_builder(
                        snippet_menu_owner.clone(),
                        SnippetMenuSurface::PostResponseScript,
                    ))
                    .completion_provider(intelligence.clone())
                    .hover_provider(intelligence)
                    .async_diagnostic_provider(move |source, cx| {
                        let diagnostics = diagnostic_intelligence.diagnostics_task(source, cx);
                        cx.background_spawn(async move {
                            diagnostics.await.into_iter().map(Into::into).collect()
                        })
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
                    .read_only(true)
                    .context_menu_builder(snippet_context_menu_builder(
                        snippet_menu_owner,
                        SnippetMenuSurface::ResponseBody,
                    )),
                window,
                cx,
            )
        });
        let (request_interchange, request_interchange_subscription) =
            Self::create_request_interchange_state(window, cx);
        let debug_overlay = cx.new(DebugOverlay::new);

        let (
            history,
            workspace,
            history_warning,
            workspace_warning,
            request_tabs,
            mut request_tabs_warning,
            history_writable,
            workspace_writable,
            request_tabs_writable,
            mut settings,
            mut settings_warning,
            settings_writable,
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
                let (request_tabs, request_tabs_warning, request_tabs_writable) =
                    match database_store.load_request_tabs() {
                        Ok(request_tabs) => (request_tabs, None, true),
                        Err(error) => (
                            RequestTabs::default(),
                            Some(format!(
                                "Request tabs could not be restored and will not be overwritten: {error}"
                            )),
                            false,
                        ),
                    };
                let (settings, settings_warning, settings_writable) =
                    match database_store.load_app_settings() {
                        Ok(settings) => (settings, None, true),
                        Err(error) => (
                            AppSettings::default(),
                            Some(format!(
                                "Settings could not be loaded and will not be overwritten: {error}"
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
                    request_tabs,
                    request_tabs_warning,
                    history_writable,
                    workspace_writable,
                    request_tabs_writable,
                    settings,
                    settings_warning,
                    settings_writable,
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
                RequestTabs::default(),
                Some(format!(
                    "Database could not be opened and request tabs will not be overwritten: {error}"
                )),
                false,
                false,
                false,
                AppSettings::default(),
                Some(format!(
                    "Database could not be opened and settings will not be overwritten: {error}"
                )),
                false,
            ),
        };
        if settings_writable {
            let mut candidate = settings.clone();
            if let Ok(true) = crate::theme::reconcile_catalog(&mut candidate.theme) {
                match database_store.save_app_settings(&candidate) {
                    Ok(()) => settings = candidate,
                    Err(error) => {
                        let warning = format!(
                            "The active CSS theme could not be added to the theme library: {error}"
                        );
                        settings_warning = Some(match settings_warning {
                            Some(existing) => format!("{existing}\n{warning}"),
                            None => warning,
                        });
                    }
                }
            }
        }
        let navigation_compact = settings.navigation_compact;
        debug_overlay.update(cx, |overlay, cx| {
            overlay.set_position(settings.metrics_position, cx);
        });
        if let Err(error) = shortcuts::apply_key_bindings(cx, &base_key_bindings, &settings) {
            let warning = format!(
                "Stored shortcuts are invalid; defaults are active and the stored settings were left untouched: {error}"
            );
            tracing::error!("{warning}");
            settings_warning = Some(warning);
            shortcuts::apply_key_bindings(cx, &base_key_bindings, &AppSettings::default())
                .expect("built-in shortcuts must be valid");
        }
        if let Some(css_source) = settings.theme.css_source.as_deref()
            && let Err(error) = crate::theme::parse_and_apply(css_source, cx)
        {
            let warning = format!(
                "Stored CSS theme is invalid; the built-in theme is active and the stored source was left untouched: {error}"
            );
            tracing::error!("{warning}");
            settings_warning = Some(match settings_warning {
                Some(existing) => format!("{existing}\n{warning}"),
                None => warning,
            });
        }
        if let Some(warning) = super::settings_actions::theme_catalog_warning(&settings) {
            settings_warning = Some(match settings_warning {
                Some(existing) => format!("{existing}\n{warning}"),
                None => warning,
            });
        }
        let mut last_persisted_request_tabs = request_tabs.clone();
        let mut request_tabs = request_tabs;
        let request_tabs_changed =
            reconcile_restored_request_tabs(&mut request_tabs, &workspace, workspace_writable);
        if request_tabs_changed && request_tabs_writable {
            match database_store.save_request_tabs(&request_tabs) {
                Ok(()) => last_persisted_request_tabs = request_tabs.clone(),
                Err(error) => {
                    request_tabs_warning =
                        Some(format!("Repaired request tabs could not be saved: {error}"));
                }
            }
        }
        let workspace_tabs = WorkspaceTabs::from_request_tabs(&request_tabs);
        let active_workspace_tab = workspace_tabs.active_tab(&request_tabs);
        let visible_workspace_tabs = workspace_tabs.visible_tabs(&request_tabs);
        let panes = PaneRoot::from_tabs(
            visible_workspace_tabs.clone(),
            visible_workspace_tabs
                .iter()
                .position(|tab| *tab == active_workspace_tab)
                .unwrap_or(0),
        );

        let selected_collection_id = request_tabs
            .active()
            .association()
            .collection_id()
            .and_then(|id| workspace.collection(id))
            .map(|collection| collection.id.clone())
            .or_else(|| {
                workspace
                    .collections
                    .first()
                    .map(|collection| collection.id.clone())
            });
        let selected_folder_id = request_tabs
            .active()
            .association()
            .folder_id()
            .filter(|folder_id| {
                selected_collection_id
                    .as_deref()
                    .and_then(|id| workspace.collection(id))
                    .is_some_and(|collection| collection.folder(folder_id).is_some())
            })
            .map(ToOwned::to_owned);
        let expanded_folder_ids = selected_collection_id
            .as_deref()
            .and_then(|collection_id| workspace.collection(collection_id))
            .and_then(|collection| {
                selected_folder_id
                    .as_deref()
                    .and_then(|folder_id| collection.folder_path_ids(folder_id).ok())
            })
            .unwrap_or_default()
            .into_iter()
            .collect();
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
        let folder_name = cx.new(|cx| InputState::new(window, cx).placeholder("Folder name"));
        let collection_delete_confirmation =
            cx.new(|cx| InputState::new(window, cx).placeholder("Type the collection name"));
        let saved_request_name =
            cx.new(|cx| InputState::new(window, cx).placeholder(DEFAULT_REQUEST_TAB_TITLE));
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
        let snippet_editor = Self::create_snippet_editor_session(
            &workspace,
            workspace_writable,
            Rc::clone(&script_variable_catalog),
            typescript_service.clone(),
            window,
            cx,
        );

        let url_subscription = cx.subscribe_in(&url, window, |this, input, event, window, cx| {
            this.track_template_input_focus(input, event);
            if matches!(event, InputEvent::Change) {
                let input = this.url.clone();
                this.schedule_template_input_refresh(&input, cx);
                this.refresh_request_dirty_part(RequestDirtyPart::Url, cx);
            }
            if matches!(event, InputEvent::PressEnter { secondary: false }) {
                this.start_request(window, cx);
            }
        });
        let method_subscription = cx.subscribe_in(&method, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                this.refresh_request_dirty_part(RequestDirtyPart::Method, cx);
            }
            if matches!(event, InputEvent::PressEnter { secondary: false }) {
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
        let pre_request_format_subscription =
            cx.subscribe_in(&pre_request_script, window, |this, _, event, window, cx| {
                if matches!(event, CodeEditorEvent::FormatRequested) {
                    let editor = this.pre_request_script.clone();
                    this.format_script_editor(editor, "pre-request", window, cx);
                }
            });
        let post_response_subscription =
            cx.subscribe(&post_response_script, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.refresh_request_dirty_part(RequestDirtyPart::PostScript, cx);
                }
            });
        let post_response_format_subscription = cx.subscribe_in(
            &post_response_script,
            window,
            |this, _, event, window, cx| {
                if matches!(event, CodeEditorEvent::FormatRequested) {
                    let editor = this.post_response_script.clone();
                    this.format_script_editor(editor, "post-response", window, cx);
                }
            },
        );
        let collection_name_subscription =
            cx.subscribe_in(&collection_name, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.rename_collection(cx);
                    this.renaming_collection_id = None;
                    cx.notify();
                }
            });
        let folder_name_subscription =
            cx.subscribe_in(&folder_name, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.finish_collection_folder_rename(cx);
                }
            });
        let saved_request_name_subscription =
            cx.subscribe(&saved_request_name, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.update_active_request_tab_title(cx);
                }
            });
        let quit_subscription = cx.on_app_quit(|this, cx| {
            this.flush_local_state(cx);
            async {}
        });
        let shortcut_target = cx.entity().downgrade();
        let shortcut_capture_subscription = cx.intercept_keystrokes(move |event, _, cx| {
            let Some(shortcut_target) = shortcut_target.upgrade() else {
                return;
            };
            if shortcut_target.read(cx).recording_shortcut_id.is_none() {
                return;
            }
            cx.stop_propagation();
            let keystroke = event.keystroke.clone();
            shortcut_target.update(cx, |this, cx| {
                this.capture_shortcut_keystroke(keystroke, cx);
            });
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
            request_pane: RequestPane::Headers,
            response_tab: ResponseTab::Body,
            pretty_body: true,
            sending: false,
            execution_stage: None,
            request_generation: 0,
            abort_handle: None,
            script_cancellation: None,
            response: None,
            response_request: None,
            response_sensitive_values: Vec::new(),
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
            navigation_compact,
            selected_collection_id,
            selected_folder_id,
            active_saved_request_id: None,
            detached_request_dirty: false,
            request_dirty: RequestDirtyState::default(),
            loaded_request_baseline: RequestTemplate::default(),
            request_notice: None,
            request_interchange,
            request_tabs,
            last_persisted_request_tabs,
            request_tab_runtime: HashMap::new(),
            request_tabs_persist_task: None,
            request_tabs_warning,
            request_tabs_writable,
            request_tab_context_target: None,
            workspace_tabs,
            panes,
            pane_editors: HashMap::new(),
            settings,
            settings_warning,
            settings_writable,
            base_key_bindings,
            recording_shortcut_id: None,
            settings_notice: None,
            theme_editors: HashMap::new(),
            snippet_editor,
            snippet_apply_generation: 0,
            snippet_apply_cancellation: None,
            selected_environment_id,
            collection_search,
            environment_search,
            expanded_collection_ids,
            expanded_folder_ids,
            renaming_collection_id: None,
            renaming_folder_id: None,
            collection_name,
            folder_name,
            collection_delete_confirmation,
            saved_request_name,
            environment_name,
            environment_variables,
            next_variable_row_id: 0,
            pending_delete: None,
            script_variable_catalog,
            typescript_service,
            template_variable_catalog,
            template_highlight_tasks: HashMap::new(),
            template_variable_hover_task: None,
            template_variable_source_hovered: None,
            template_variable_popover_hovered: false,
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
                pre_request_format_subscription,
                post_response_subscription,
                post_response_format_subscription,
                collection_name_subscription,
                folder_name_subscription,
                saved_request_name_subscription,
                request_interchange_subscription,
                quit_subscription,
                shortcut_capture_subscription,
            ],
        };
        this.apply_code_editor_settings(window, cx);
        this.push_header_row("", "", true, window, cx);
        this.refresh_variable_intelligence(cx);
        this.loaded_request_baseline = this.request_template(cx);
        this.request_dirty.clear();
        this.restore_active_request_tab(window, cx);
        this
    }
}
