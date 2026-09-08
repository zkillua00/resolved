use super::*;

const EXPORT_GROUPS: &[&str] = &[
    "Command line",
    "API specifications",
    "HTTP Client",
    "JavaScript",
    "Java",
    "Go",
    "C#",
    "Rust",
    "C++",
    "PHP",
    "Kotlin",
];

pub(super) const REQUEST_INTERCHANGE_PANEL_WIDTH: f32 = 420.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RequestInterchangeTab {
    Import,
    Export,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ImportPreview {
    Empty,
    Ready {
        source_format: String,
        request_count: usize,
    },
    Error(String),
}

pub(super) struct RequestInterchangeState {
    open: bool,
    tab: RequestInterchangeTab,
    selected_format: InterchangeFormat,
    import_editor: Entity<CodeEditor>,
    export_editor: Entity<CodeEditor>,
    import_preview: ImportPreview,
    export_error: Option<String>,
    loaded_import: Option<(String, ImportBundle)>,
}

impl RequestInterchangeState {
    fn new(import_editor: Entity<CodeEditor>, export_editor: Entity<CodeEditor>) -> Self {
        Self {
            open: false,
            tab: RequestInterchangeTab::Import,
            selected_format: InterchangeFormat::Curl,
            import_editor,
            export_editor,
            import_preview: ImportPreview::Empty,
            export_error: None,
            loaded_import: None,
        }
    }
}

fn build_export_format_menu(
    mut menu: PopupMenu,
    owner: gpui::WeakEntity<ApiTester>,
    selected_format: InterchangeFormat,
) -> PopupMenu {
    menu = menu.min_w(px(280.)).max_h(px(520.)).scrollable(true);
    for group in EXPORT_GROUPS {
        menu = menu.label(*group);
        for format in InterchangeFormat::ALL
            .iter()
            .copied()
            .filter(|format| format.group() == *group)
        {
            let item_owner = owner.clone();
            menu = menu.item(
                PopupMenuItem::new(format.label())
                    .checked(format == selected_format)
                    .on_click(move |_, window, cx| {
                        if let Some(owner) = item_owner.upgrade() {
                            owner.update(cx, |this, cx| {
                                this.select_request_export_format(format, window, cx);
                            });
                        }
                    }),
            );
        }
    }
    menu
}

fn export_code_language(format: InterchangeFormat) -> CodeLanguage {
    match format {
        InterchangeFormat::Curl | InterchangeFormat::Wget => CodeLanguage::Shell,
        InterchangeFormat::PowerShell => CodeLanguage::from("powershell"),
        InterchangeFormat::OpenApi | InterchangeFormat::AsyncApi => CodeLanguage::Yaml,
        InterchangeFormat::IntelliJHttp => CodeLanguage::from("http"),
        InterchangeFormat::JavaScriptFetch
        | InterchangeFormat::JavaScriptAxios
        | InterchangeFormat::JavaScriptJquery => CodeLanguage::JavaScript,
        InterchangeFormat::JavaHttpClient | InterchangeFormat::JavaOkHttp => {
            CodeLanguage::from("java")
        }
        InterchangeFormat::GoNetHttp | InterchangeFormat::GoResty => CodeLanguage::from("go"),
        InterchangeFormat::CSharpHttpClient | InterchangeFormat::CSharpRestSharp => {
            CodeLanguage::from("csharp")
        }
        InterchangeFormat::RustReqwest | InterchangeFormat::RustUreq => CodeLanguage::Rust,
        InterchangeFormat::CppBoostBeast | InterchangeFormat::CppLibcurl => {
            CodeLanguage::from("cpp")
        }
        InterchangeFormat::PhpCurl | InterchangeFormat::PhpGuzzle => CodeLanguage::from("php"),
        InterchangeFormat::KotlinKtor
        | InterchangeFormat::KotlinOkHttp
        | InterchangeFormat::KotlinJavaHttpClient => CodeLanguage::from("kotlin"),
    }
}

impl ApiTester {
    pub(super) fn create_request_interchange_state(
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (RequestInterchangeState, Subscription) {
        let import_editor = cx.new(|cx| {
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(CodeLanguage::Plain)
                    .placeholder("Paste Postman JSON, Swagger 2.0, OpenAPI, cURL, or request code")
                    .rows(18)
                    .soft_wrap(false)
                    .line_numbers(true)
                    .framed(false),
                window,
                cx,
            )
        });
        let export_editor = cx.new(|cx| {
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(CodeLanguage::Shell)
                    .placeholder("Generated request code")
                    .rows(24)
                    .soft_wrap(false)
                    .line_numbers(true)
                    .read_only(true)
                    .framed(false),
                window,
                cx,
            )
        });
        let import_subscription =
            cx.subscribe(&import_editor, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.refresh_request_import_preview(cx);
                    cx.notify();
                }
            });
        (
            RequestInterchangeState::new(import_editor, export_editor),
            import_subscription,
        )
    }

    pub(super) fn open_request_import_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sending {
            self.request_notice =
                Some("Finish or cancel the active request before importing.".to_owned());
            cx.notify();
            return;
        }
        self.request_interchange.open = true;
        self.request_interchange.tab = RequestInterchangeTab::Import;
        self.request_interchange
            .import_editor
            .read(cx)
            .focus_handle(cx)
            .focus(window);
        cx.notify();
    }

    pub(super) fn open_request_export_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sending || self.workspace_switch_status.busy() {
            return;
        }
        if self.request_tabs.active().template().is_websocket() {
            self.request_notice = Some("WebSocket export is not available yet.".to_owned());
            cx.notify();
            return;
        }
        self.request_interchange.open = true;
        self.request_interchange.tab = RequestInterchangeTab::Export;
        self.sync_request_export_preview(window, cx);
        cx.notify();
    }

    fn select_request_export_format(
        &mut self,
        format: InterchangeFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_interchange.selected_format = format;
        self.sync_request_export_preview(window, cx);
        cx.notify();
    }

    pub(super) fn sync_request_export_preview(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.request_interchange.open
            || self.request_interchange.tab != RequestInterchangeTab::Export
        {
            return;
        }
        let format = self.request_interchange.selected_format;
        let result = self.active_export(format, cx);
        let editor = self.request_interchange.export_editor.clone();
        let (source, language, error) = match result {
            Ok(source) => (source, export_code_language(format), None),
            Err(error) => (
                format!("Export unavailable\n\n{error}"),
                CodeLanguage::Plain,
                Some(error),
            ),
        };
        self.request_interchange.export_error = error;
        let current_source = editor.read(cx).value(cx);
        let current_language = editor.read(cx).language().clone();
        if current_source.as_ref() != source || current_language != language {
            editor.update(cx, |editor, cx| {
                if editor.language() != &language {
                    editor.set_language(language, cx);
                }
                editor.set_value(source, window, cx);
            });
        }
    }

    fn request_import_bundle(&self, source: &str) -> Result<ImportBundle, String> {
        if let Some((loaded_source, bundle)) = &self.request_interchange.loaded_import
            && loaded_source == source
        {
            return Ok(bundle.clone());
        }
        import_requests(source).map_err(|error| error.to_string())
    }

    fn preview_imported_files(
        &mut self,
        bundle: ImportBundle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace_switch_status.busy() {
            return;
        }
        let source = format!(
            "Loaded {} requests from {}.\n\n{}\n\nChoose Open tabs or Import and save below.\nPaste new source to replace this import.",
            bundle.requests.len(),
            bundle.source_format,
            bundle
                .collections
                .iter()
                .map(|c| format!(
                    "{} · {} requests · {} folders",
                    c.name,
                    c.requests.len(),
                    c.folders.len()
                ))
                .collect::<Vec<_>>()
                .join("\n")
        );
        self.request_interchange
            .import_editor
            .update(cx, |editor, cx| {
                editor.set_value(source.clone(), window, cx)
            });
        self.request_interchange.loaded_import = Some((source, bundle));
        self.request_interchange.open = true;
        self.request_interchange.tab = RequestInterchangeTab::Import;
        self.refresh_request_import_preview(cx);
        cx.notify();
    }

    fn request_import_details(&self, cx: &Context<Self>) -> String {
        let source = self
            .request_interchange
            .import_editor
            .read(cx)
            .value(cx)
            .to_string();
        let Ok(mut bundle) = self.request_import_bundle(&source) else {
            return String::new();
        };
        bundle.ensure_collection("Imported requests");
        let mut details = format!(
            "Save to {}: {}",
            self.active_workspace_name(),
            bundle
                .collections
                .iter()
                .map(|c| format!("{} ({} requests)", c.name, c.requests.len()))
                .collect::<Vec<_>>()
                .join(", ")
        );
        for warning in bundle.warnings {
            details.push('\n');
            details.push_str(&warning);
        }
        if !self.can_save_request_import() {
            details.push_str(
                "\nSaving requires collection and request creation access in a writable workspace.",
            );
        }
        details
    }

    fn can_save_request_import(&self) -> bool {
        !self.sending
            && !self.workspace_switch_status.busy()
            && self.can_create_collection_content()
            && self.can_create_request_content()
    }

    fn save_request_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_save_request_import() {
            return;
        }
        let source = self
            .request_interchange
            .import_editor
            .read(cx)
            .value(cx)
            .to_string();
        let result = self
            .request_import_bundle(&source)
            .and_then(|bundle| build_import_workspace(&self.workspace, bundle));
        let candidate = match result {
            Ok(candidate) => candidate,
            Err(error) => {
                self.request_interchange.import_preview = ImportPreview::Error(error);
                cx.notify();
                return;
            }
        };
        if !self.workspace_writable {
            self.save_request_import_on_upstream(
                candidate.collections[self.workspace.collections.len()..].to_vec(),
                window,
                cx,
            );
            return;
        }
        let count = candidate.collections[self.workspace.collections.len()..]
            .iter()
            .map(|c| c.requests.len())
            .sum::<usize>();
        let ids = candidate.collections[self.workspace.collections.len()..]
            .iter()
            .map(|c| c.id.clone())
            .collect::<Vec<_>>();
        match self.commit_workspace(candidate) {
            Ok(()) => {
                self.expanded_collection_ids.extend(ids);
                self.finish_request_import_save(count, window, cx);
            }
            Err(error) => {
                self.request_interchange.import_preview = ImportPreview::Error(error);
                cx.notify();
            }
        }
    }

    fn finish_request_import_save(
        &mut self,
        count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_interchange.open = false;
        self.request_interchange.loaded_import = None;
        self.request_interchange.import_preview = ImportPreview::Empty;
        self.request_interchange
            .import_editor
            .update(cx, |editor, cx| editor.set_value("", window, cx));
        self.request_notice = Some(format!("Saved {count} imported requests to collections."));
        self.refresh_variable_intelligence(cx);
        cx.notify();
    }

    fn save_request_import_on_upstream(
        &mut self,
        collections: Vec<Collection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = match self.active_upstream_workspace() {
            Ok(target) => target,
            Err(error) => {
                self.fail_remote_workspace_write(error, cx);
                return;
            }
        };
        self.workspace_switch_generation = self.workspace_switch_generation.wrapping_add(1);
        let generation = self.workspace_switch_generation;
        self.workspace_switch_status = WorkspaceSwitchStatus::Loading;
        self.request_interchange.open = false;
        let vault = self.credential_vault.clone();
        let client = self.upstream_client.clone();
        let runtime = Arc::clone(&self.runtime);
        let task_target = target.clone();
        let task = self.runtime.spawn(async move {
            let mut saved = Vec::new();
            let result: Result<(), String> = async {
                let upstream_id = task_target.upstream_id.clone();
                let credential = runtime
                    .spawn_blocking(move || vault.load_upstream(&upstream_id))
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "Log in to this server again.".to_owned())?;
                if credential.expires_at <= Utc::now() {
                    return Err("Log in to this server again.".into());
                }
                for collection in collections {
                    let created = create_upstream_collection(
                        &client,
                        &task_target.base_url,
                        credential.bearer_token(),
                        &task_target.workspace_id,
                        &collection.name,
                        None,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                    if created.workspace_id != task_target.workspace_id
                        || created.parent_collection_id.is_some()
                    {
                        return Err("Server returned an unexpected collection location.".into());
                    }
                    saved.push(Collection {
                        id: created.id.clone(),
                        name: created.name,
                        created_by: created.created_by.map(Into::into),
                        folders: Vec::new(),
                        requests: Vec::new(),
                    });
                    let current = saved.last_mut().unwrap();
                    let mut folders = std::collections::HashMap::<String, String>::new();
                    for folder in collection.folders {
                        let parent = folder
                            .parent_folder_id
                            .as_ref()
                            .map(|id| {
                                folders
                                    .get(id)
                                    .cloned()
                                    .ok_or_else(|| "Missing imported parent folder.".to_owned())
                            })
                            .transpose()?;
                        let parent_id = parent.as_deref().unwrap_or(current.id.as_str());
                        let created = create_upstream_collection(
                            &client,
                            &task_target.base_url,
                            credential.bearer_token(),
                            &task_target.workspace_id,
                            &folder.name,
                            Some(parent_id),
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                        if created.workspace_id != task_target.workspace_id
                            || created.parent_collection_id.as_deref() != Some(parent_id)
                        {
                            return Err("Server returned an unexpected folder location.".into());
                        }
                        folders.insert(folder.id, created.id.clone());
                        current.folders.push(CollectionFolder {
                            id: created.id,
                            name: created.name,
                            created_by: created.created_by.map(Into::into),
                            parent_folder_id: parent,
                        });
                    }
                    for request in collection.requests {
                        let folder_id = request
                            .folder_id
                            .as_ref()
                            .map(|id| {
                                folders
                                    .get(id)
                                    .cloned()
                                    .ok_or_else(|| "Missing imported request folder.".to_owned())
                            })
                            .transpose()?;
                        let destination = folder_id.as_deref().unwrap_or(current.id.as_str());
                        let created = create_upstream_saved_request(
                            &client,
                            &task_target.base_url,
                            credential.bearer_token(),
                            &task_target.workspace_id,
                            destination,
                            &request.name,
                            &request.definition,
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                        if created.collection_id != destination {
                            return Err("Server returned an unexpected request location.".into());
                        }
                        current.requests.push(created.into_local(folder_id));
                    }
                }
                Ok(())
            }
            .await;
            (saved, result)
        });
        self.workspace_switch_abort_handle = Some(task.abort_handle());
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.workspace_switch_generation != generation {
                    return;
                }
                this.workspace_switch_abort_handle = None;
                let expected = WorkspaceProviderId::Upstream {
                    upstream_id: target.upstream_id.clone(),
                    workspace_id: target.workspace_id.clone(),
                };
                if this.workspace_providers.active_id() != &expected {
                    return;
                }
                let (saved, outcome) = match result {
                    Ok(value) => value,
                    Err(error) => {
                        this.fail_remote_workspace_write(
                            format!("Import interrupted. Refresh the workspace before retrying: {error}"), cx,
                        );
                        return;
                    }
                };
                let count = saved.iter().map(|collection| collection.requests.len()).sum::<usize>();
                let mut candidate = this.workspace.clone();
                this.expanded_collection_ids.extend(saved.iter().map(|collection| collection.id.clone()));
                candidate.collections.extend(saved);
                if let Err(error) = candidate.validate() {
                    this.fail_remote_workspace_write(
                        format!("Refresh the workspace to inspect the saved import: {error}"), cx,
                    );
                    return;
                }
                this.replace_workspace(candidate.clone());
                this.workspace_providers.register(Arc::new(RemoteWorkspaceProvider::new(
                    this.database_store.clone(), target.upstream_id, target.workspace_id, candidate,
                )));
                this.workspace_switch_status = WorkspaceSwitchStatus::Idle;
                match outcome {
                    Ok(()) => this.finish_request_import_save(count, window, cx),
                    Err(error) => {
                        let message = format!("Import stopped after saving {count} requests. Created collections remain saved; retrying creates new collections. {error}");
                        this.request_interchange.open = true;
                        this.request_interchange.import_preview = ImportPreview::Error(message.clone());
                        this.fail_remote_workspace_write(message, cx);
                    }
                }
            });
        }).detach();
    }

    fn refresh_request_import_preview(&mut self, cx: &mut Context<Self>) {
        let source = self
            .request_interchange
            .import_editor
            .read(cx)
            .value(cx)
            .to_string();
        self.request_interchange.import_preview = if source.trim().is_empty() {
            ImportPreview::Empty
        } else {
            match self.request_import_bundle(&source) {
                Ok(bundle) => {
                    let preview = ImportPreview::Ready {
                        source_format: bundle.source_format.clone(),
                        request_count: bundle.requests.len(),
                    };
                    self.request_interchange.loaded_import = Some((source, bundle));
                    preview
                }
                Err(error) => ImportPreview::Error(error.to_string()),
            }
        };
    }

    fn paste_request_import_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace_switch_status.busy() {
            return;
        }
        let Some(source) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.request_interchange.import_preview =
                ImportPreview::Error("The clipboard does not contain text.".to_owned());
            cx.notify();
            return;
        };
        self.request_interchange
            .import_editor
            .update(cx, |editor, cx| editor.set_value(source, window, cx));
        self.refresh_request_import_preview(cx);
        self.request_interchange
            .import_editor
            .read(cx)
            .focus_handle(cx)
            .focus(window);
        cx.notify();
    }

    fn import_request_editor_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending || self.workspace_switch_status.busy() {
            return;
        }
        let source = self
            .request_interchange
            .import_editor
            .read(cx)
            .value(cx)
            .to_string();
        match self.request_import_bundle(&source) {
            Ok(bundle) => self.open_imported_requests(bundle, window, cx),
            Err(error) => {
                self.request_interchange.import_preview = ImportPreview::Error(error.to_string());
                cx.notify();
            }
        }
    }

    fn import_request_paths(
        &mut self,
        paths: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sending || self.workspace_switch_status.busy() || paths.is_empty() {
            return;
        }
        match import_request_files(paths) {
            Ok(bundle) => self.preview_imported_files(bundle, window, cx),
            Err(error) => {
                self.request_interchange.import_preview = ImportPreview::Error(error.clone());
                self.request_notice = Some(format!("Import failed: {error}"));
                cx.notify();
            }
        }
    }

    pub(super) fn render_request_interchange_panel(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.request_interchange.open {
            return None;
        }
        let tab = self.request_interchange.tab;
        let content = match tab {
            RequestInterchangeTab::Import => self.render_request_import_panel(cx),
            RequestInterchangeTab::Export => self.render_request_export_panel(cx),
        };
        let (panel_title, close_tooltip) = match tab {
            RequestInterchangeTab::Import => ("Import requests", "Close import"),
            RequestInterchangeTab::Export => ("Export request", "Close export"),
        };
        Some(
            v_flex()
                .debug_selector(|| "request-interchange-panel".to_owned())
                .w_full()
                .h_full()
                .flex_shrink_0()
                .overflow_hidden()
                .border_l_1()
                .border_color(cx.api_outline_variant())
                .bg(cx.api_surface())
                .shadow_lg()
                .child(
                    h_flex()
                        .h(px(52.))
                        .flex_shrink_0()
                        .px_4()
                        .justify_between()
                        .border_b_1()
                        .border_color(cx.api_outline_variant())
                        .child(div().text_base().font_semibold().child(panel_title))
                        .child(
                            Button::new("close-request-interchange")
                                .icon(IconName::Close)
                                .small()
                                .ghost()
                                .tooltip(close_tooltip)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.request_interchange.open = false;
                                    cx.notify();
                                })),
                        ),
                )
                .child(content)
                .into_any_element(),
        )
    }

    fn render_request_import_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let (status, can_import, _import_label) = match &self.request_interchange.import_preview {
            ImportPreview::Empty => (
                h_flex()
                    .min_h(px(42.))
                    .px_3()
                    .rounded_lg()
                    .bg(cx.api_surface_low())
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Paste a request and Resolved will detect the format before importing.")
                    .into_any_element(),
                false,
                "Import request".to_owned(),
            ),
            ImportPreview::Ready {
                source_format,
                request_count,
            } => (
                h_flex()
                    .min_h(px(42.))
                    .px_3()
                    .gap_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.theme().success.opacity(0.45))
                    .bg(cx.theme().success.opacity(0.07))
                    .text_xs()
                    .text_color(cx.theme().success)
                    .child(Icon::new(IconName::CircleCheck).xsmall())
                    .child(format!(
                        "Detected {source_format} · {request_count} {} ready",
                        if *request_count == 1 {
                            "request"
                        } else {
                            "requests"
                        }
                    ))
                    .into_any_element(),
                true,
                if *request_count == 1 {
                    "Import request".to_owned()
                } else {
                    format!("Import {request_count} requests")
                },
            ),
            ImportPreview::Error(error) => (
                h_flex()
                    .min_h(px(42.))
                    .max_h(px(70.))
                    .overflow_hidden()
                    .px_3()
                    .gap_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.theme().danger.opacity(0.45))
                    .bg(cx.theme().danger.opacity(0.07))
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(Icon::new(IconName::CircleX).xsmall())
                    .child(error.clone())
                    .into_any_element(),
                false,
                "Import request".to_owned(),
            ),
        };

        v_flex()
            .debug_selector(|| "request-interchange-import".to_owned())
            .flex_1()
            .min_h_0()
            .p_4()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().font_semibold().child("Paste request data"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                "Commands, specifications, HTTP Client files, and request source are detected automatically. Dynamic URL expressions become {{variables}}.",
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(190.))
                    .overflow_hidden()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.api_outline_variant())
                    .child(self.request_interchange.import_editor.clone()),
            )
            .child(status)
            .child(
                v_flex()
                    .id("request-import-drop-zone")
                    .h(px(78.))
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .rounded_lg()
                    .border_1()
                    .border_dashed()
                    .border_color(cx.api_outline_variant())
                    .bg(cx.api_surface_low())
                    .can_drop(|value, _, _| value.downcast_ref::<ExternalPaths>().is_some())
                    .drag_over::<ExternalPaths>(|style, _, _, cx| {
                        style
                            .border_color(cx.theme().primary)
                            .bg(cx.theme().primary.opacity(0.08))
                    })
                    .on_drop(cx.listener(
                        |this, paths: &ExternalPaths, window, cx| {
                            this.import_request_paths(paths.paths(), window, cx);
                        },
                    ))
                    .child(div().text_sm().font_semibold().child("Drop request files here"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("UTF-8 text · up to 8 MiB total"),
                    ),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .justify_between()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("paste-request-import-source")
                                    .label("Paste")
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.paste_request_import_source(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("choose-request-import-files")
                                    .icon(IconName::FolderOpen)
                                    .label("Choose files")
                                    .small()
                                    .outline()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.choose_request_import_files(window, cx);
                                    })),
                            ),
                    )
                    .child(
                        Button::new("import-detected-requests")
                            .label("Open tabs")
                            .small()
                            .outline()
                            .disabled(!can_import || self.sending || self.workspace_switch_status.busy())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.import_request_editor_source(window, cx);
                            })),
                    ),
            )
            .child(
                Button::new("import-and-save-requests")
                    .debug_selector(|| "import-and-save-requests".to_owned())
                    .label("Import and save")
                    .primary()
                    .disabled(!can_import || !self.can_save_request_import())
                    .on_click(cx.listener(|this, _, window, cx| this.save_request_import(window, cx))),
            )
            .child(div().id("import-save-details").max_h(px(110.)).overflow_y_scroll().text_xs().text_color(cx.theme().muted_foreground).child(self.request_import_details(cx)))
            .into_any_element()
    }

    fn render_request_export_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let format = self.request_interchange.selected_format;
        let owner = cx.entity().downgrade();
        let export_available = self.request_interchange.export_error.is_none();

        v_flex()
            .debug_selector(|| "request-interchange-export".to_owned())
            .flex_1()
            .min_h_0()
            .p_4()
            .gap_3()
            .child(
                h_flex()
                    .gap_3()
                    .justify_between()
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_sm()
                            .font_semibold()
                            .child("Format"),
                    )
                    .child(
                        Button::new("request-export-format")
                            .label(format.label())
                            .small()
                            .outline()
                            .w(px(280.))
                            .dropdown_caret(true)
                            .dropdown_menu(move |menu, _, _| {
                                build_export_format_menu(menu, owner.clone(), format)
                            })
                            .anchor(Corner::TopRight),
                    ),
            )
            .when_some(
                self.request_interchange.export_error.clone(),
                |this, error| {
                    this.child(
                        h_flex()
                            .min_h(px(42.))
                            .px_3()
                            .gap_2()
                            .rounded_lg()
                            .border_1()
                            .border_color(cx.theme().danger.opacity(0.45))
                            .bg(cx.theme().danger.opacity(0.07))
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child(Icon::new(IconName::CircleX).xsmall())
                            .child(error),
                    )
                },
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(260.))
                    .overflow_hidden()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.api_outline_variant())
                    .child(self.request_interchange.export_editor.clone()),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("save-request-export")
                            .icon(IconName::FolderOpen)
                            .label("Save file")
                            .small()
                            .outline()
                            .disabled(!export_available)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.save_active_request_as(format, window, cx);
                            })),
                    )
                    .child(
                        Button::new("copy-request-export")
                            .icon(IconName::Copy)
                            .label("Copy")
                            .small()
                            .primary()
                            .disabled(!export_available)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.copy_active_request_as(format, cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}

impl ApiTester {
    pub(super) fn choose_request_import_files(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sending {
            self.request_notice =
                Some("Finish or cancel the active request before importing.".to_owned());
            cx.notify();
            return;
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Import requests".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let result = import_request_files(&paths);
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(bundle) => this.preview_imported_files(bundle, window, cx),
                Err(error) => {
                    this.request_interchange.import_preview = ImportPreview::Error(error.clone());
                    this.request_notice = Some(format!("Import failed: {error}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn open_imported_requests(
        &mut self,
        bundle: ImportBundle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if bundle.requests.is_empty() {
            self.request_notice = Some("The import contains no requests.".to_owned());
            cx.notify();
            return;
        }
        self.request_interchange.open = false;
        self.request_interchange.loaded_import = None;
        self.request_interchange.import_preview = ImportPreview::Empty;
        self.request_interchange
            .import_editor
            .update(cx, |editor, cx| editor.set_value("", window, cx));
        if self.workspace_tabs.active() == ActiveWorkspaceTab::Settings {
            self.cancel_shortcut_recording(cx);
        }
        let backing_id = (!self.request_is_dirty()
            && self.request_tabs.active().is_pristine_scratch())
        .then(|| self.request_tabs.active_tab_id().clone());
        let welcome_backing = self.workspace_tabs.take_welcome_request_tab_id();
        if welcome_backing.is_some() {
            self.request_tabs.dismiss_welcome();
        }
        let discard_backing = welcome_backing.or(backing_id);
        if discard_backing.is_none() {
            self.snapshot_active_request_tab(cx);
        }

        self.workspace_tabs.activate_request();
        self.sidebar_tab = SidebarTab::Collections;
        let association = RequestTabAssociation::new(
            self.selected_folder_id.clone(),
            self.selected_collection_id.clone(),
            None,
        );
        let imported_count = bundle.requests.len();
        let source_format = bundle.source_format;
        let mut last_tab_id = None;
        for imported in bundle.requests {
            let tab_id = self.request_tabs.open_unsaved(
                imported.name,
                imported.template,
                association.clone(),
            );
            self.request_tab_runtime
                .insert(tab_id.as_str().to_owned(), RequestTabRuntime::default());
            last_tab_id = Some(tab_id);
        }
        if let Some(backing_id) = discard_backing {
            self.request_tabs
                .close_tabs(std::slice::from_ref(&backing_id));
            self.request_tab_runtime.remove(backing_id.as_str());
        }
        if let Some(last_tab_id) = last_tab_id {
            self.expand_request_tab_group_for(&last_tab_id);
        }
        self.hide_preview(cx);
        self.restore_active_request_tab(window, cx);
        self.request_notice = Some(if imported_count == 1 {
            format!("Imported 1 request from {source_format} into a new tab.")
        } else {
            format!("Imported {imported_count} requests from {source_format} into new tabs.")
        });
        self.persist_request_tabs_now(cx);
        cx.notify();
    }

    fn active_export(&self, format: InterchangeFormat, cx: &App) -> Result<String, String> {
        let title = self.saved_request_name.read(cx).value().trim().to_owned();
        let title = if title.is_empty() {
            self.request_tabs.active().display_title().to_owned()
        } else {
            title
        };
        export_request(format, &title, &self.request_template(cx))
            .map_err(|error| error.to_string())
    }

    pub(super) fn copy_active_request_as(
        &mut self,
        format: InterchangeFormat,
        cx: &mut Context<Self>,
    ) {
        match self.active_export(format, cx) {
            Ok(source) => {
                cx.write_to_clipboard(ClipboardItem::new_string(source));
                self.request_notice = Some(format!("Copied {} to the clipboard.", format.label()));
            }
            Err(error) => self.request_notice = Some(format!("Export failed: {error}")),
        }
        cx.notify();
    }

    pub(super) fn save_active_request_as(
        &mut self,
        format: InterchangeFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let source = match self.active_export(format, cx) {
            Ok(source) => source,
            Err(error) => {
                self.request_notice = Some(format!("Export failed: {error}"));
                cx.notify();
                return;
            }
        };
        let base_name = safe_export_file_name(self.request_tabs.active().display_title());
        let suggested_name = format!("{base_name}.{}", format.file_extension());
        let directory = dirs::download_dir()
            .or_else(dirs::document_dir)
            .unwrap_or_else(std::env::temp_dir);
        let receiver = cx.prompt_for_new_path(&directory, Some(&suggested_name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = receiver.await else {
                return;
            };
            let result = std::fs::write(&path, source)
                .map_err(|error| format!("{}: {error}", path.display()));
            let _ = this.update(cx, |this, cx| {
                this.request_notice = Some(match result {
                    Ok(()) => format!("Exported {} to {}.", format.label(), path.display()),
                    Err(error) => format!("Export failed: {error}"),
                });
                cx.notify();
            });
        })
        .detach();
    }
}

fn build_import_workspace(
    workspace: &Workspace,
    mut bundle: ImportBundle,
) -> Result<Workspace, String> {
    bundle.ensure_collection("Imported requests");
    let mut candidate = workspace.clone();
    for collection in bundle.collections {
        let id = candidate
            .create_collection(collection.name)
            .map_err(|e| e.to_string())?;
        let mut folders = std::collections::HashMap::<Vec<String>, String>::new();
        for path in collection
            .folders
            .iter()
            .chain(collection.requests.iter().map(|(_, path)| path))
        {
            for length in 1..=path.len() {
                let key = path[..length].to_vec();
                if folders.contains_key(&key) {
                    continue;
                }
                let parent = folders.get(&path[..length - 1]).map(String::as_str);
                let folder_id = candidate
                    .create_collection_folder(&id, parent, &path[length - 1])
                    .map_err(|e| e.to_string())?;
                folders.insert(key, folder_id);
            }
        }
        for (index, path) in collection.requests {
            let request = bundle
                .requests
                .get(index)
                .ok_or_else(|| "Invalid imported request index".to_owned())?;
            candidate
                .create_saved_request_in_folder(
                    &id,
                    folders.get(&path).map(String::as_str),
                    &request.name,
                    request.template.clone(),
                )
                .map_err(|e| e.to_string())?;
        }
    }
    candidate.validate().map_err(|e| e.to_string())?;
    Ok(candidate)
}

fn import_request_files(paths: &[PathBuf]) -> Result<ImportBundle, String> {
    let mut formats = Vec::new();
    let mut requests = Vec::new();
    let mut collections = Vec::new();
    let mut warnings = Vec::new();
    let mut total_bytes = 0_u64;
    for path in paths {
        let metadata =
            std::fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
        total_bytes = total_bytes
            .checked_add(metadata.len())
            .ok_or_else(|| "the selected files are too large to import safely".to_owned())?;
        if metadata.len() > MAX_INTERCHANGE_BYTES as u64
            || total_bytes > MAX_INTERCHANGE_BYTES as u64
        {
            return Err(format!(
                "the selected import is larger than the {MAX_INTERCHANGE_BYTES} byte safety limit"
            ));
        }
        let source = std::fs::read_to_string(path)
            .map_err(|error| format!("{} is not readable UTF-8 text: {error}", path.display()))?;
        let mut bundle =
            import_requests(&source).map_err(|error| format!("{}: {error}", path.display()))?;
        if !formats.contains(&bundle.source_format) {
            formats.push(bundle.source_format.clone());
        }
        bundle.ensure_collection(
            path.file_stem()
                .and_then(|v| v.to_str())
                .unwrap_or("Imported requests"),
        );
        for mut collection in bundle.collections {
            for (index, _) in &mut collection.requests {
                *index += requests.len();
            }
            collections.push(collection);
        }
        for warning in bundle.warnings {
            if !warnings.contains(&warning) {
                warnings.push(warning);
            }
        }
        requests.extend(bundle.requests);
        if requests.len() > 256 {
            return Err("the selected files contain more than 256 requests".to_owned());
        }
    }
    if requests.is_empty() {
        return Err("the selected files contain no requests".to_owned());
    }
    Ok(ImportBundle {
        collections,
        warnings,
        source_format: formats.join(" + "),
        requests,
    })
}

fn safe_export_file_name(value: &str) -> String {
    let mut result = String::new();
    let mut previous_separator = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
            result.push(character.to_ascii_lowercase());
            previous_separator = false;
        } else if !previous_separator && !result.is_empty() {
            result.push('-');
            previous_separator = true;
        }
    }
    while result.ends_with('-') {
        result.pop();
    }
    if result.is_empty() {
        "request".to_owned()
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, px, size};
    use gpui_component::highlighter::LanguageRegistry;

    #[test]
    fn every_code_export_resolves_to_a_real_highlight_query() {
        crate::syntax_languages::register();

        for format in InterchangeFormat::ALL
            .iter()
            .copied()
            .filter(|format| *format != InterchangeFormat::IntelliJHttp)
        {
            let language = export_code_language(format);
            let config = LanguageRegistry::singleton()
                .language(language.as_str())
                .unwrap_or_else(|| panic!("{} has no registered syntax language", format.label()));
            assert!(
                !config.highlights.is_empty(),
                "{} resolves to a grammar without a highlight query",
                format.label()
            );
        }
    }

    #[test]
    fn export_file_names_are_safe_and_stable() {
        assert_eq!(safe_export_file_name("Create user / v1"), "create-user-v1");
        assert_eq!(safe_export_file_name("###"), "request");
    }

    #[gpui::test]
    fn imports_reuse_only_a_clean_backing_tab_and_never_overwrite_a_draft(cx: &mut TestAppContext) {
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
            Root::new(view, window, cx)
        });
        let app = app.expect("capture app entity");
        cx.update(|window, _| window.activate_window());
        cx.simulate_resize(size(px(1_200.), px(800.)));

        let first = import_requests("curl https://api.example.com/first").unwrap();
        cx.update(|window, cx| {
            app.update(cx, |app, cx| app.open_imported_requests(first, window, cx));
        });
        assert_eq!(
            cx.update(|_, cx| app.read(cx).request_tabs.tabs().len()),
            1,
            "the pristine Welcome backing tab should be reused"
        );

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.url.update(cx, |input, cx| {
                    input.set_value("https://keep.example.com/draft", window, cx);
                });
            });
        });
        let second = import_requests("curl https://api.example.com/second").unwrap();
        cx.update(|window, cx| {
            app.update(cx, |app, cx| app.open_imported_requests(second, window, cx));
        });

        cx.update(|_, cx| {
            let app = app.read(cx);
            assert_eq!(app.request_tabs.tabs().len(), 2);
            assert_eq!(
                app.request_tabs.tabs()[0].template().request.url,
                "https://keep.example.com/draft"
            );
            assert_eq!(
                app.request_tabs.active().template().request.url,
                "https://api.example.com/second"
            );
        });
    }

    #[gpui::test]
    fn interchange_panels_detect_imports_and_keep_generated_code_live(cx: &mut TestAppContext) {
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
            Root::new(view, window, cx)
        });
        let app = app.expect("capture app entity");
        cx.update(|window, _| window.activate_window());
        cx.simulate_resize(size(px(1_200.), px(800.)));

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_request_import_panel(window, cx);
                let editor = app.request_interchange.import_editor.clone();
                editor.update(cx, |editor, cx| {
                    editor.set_value("curl https://api.example.com/users", window, cx);
                });
                app.refresh_request_import_preview(cx);
            });
        });
        cx.run_until_parked();

        cx.update(|_, cx| {
            let app = app.read(cx);
            assert!(matches!(
                app.request_interchange.import_preview,
                ImportPreview::Ready {
                    request_count: 1,
                    ..
                }
            ));
        });
        let panel_bounds = cx
            .debug_bounds("request-interchange-panel")
            .expect("the transfer drawer should be rendered");
        assert!(panel_bounds.size.width >= px(400.));

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_request_export_panel(window, cx);
            });
        });
        cx.run_until_parked();
        let curl_source = cx.update(|_, cx| {
            app.read(cx)
                .request_interchange
                .export_editor
                .read(cx)
                .value(cx)
                .to_string()
        });
        assert!(curl_source.contains("curl --request"));

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.select_request_export_format(InterchangeFormat::JavaScriptFetch, window, cx);
            });
        });
        let javascript_source = cx.update(|_, cx| {
            let app = app.read(cx);
            assert_eq!(
                app.request_interchange.selected_format,
                InterchangeFormat::JavaScriptFetch
            );
            app.request_interchange
                .export_editor
                .read(cx)
                .value(cx)
                .to_string()
        });
        assert!(javascript_source.contains("await fetch"));
    }
    #[test]
    fn importing_multiple_files_keeps_roots_and_request_indices() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.json");
        let second = directory.path().join("second.yaml");
        std::fs::write(&first, r#"{"info":{"name":"Shop","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},"item":[{"name":"Admin","item":[{"name":"Empty","item":[]},{"name":"List","request":"https://example.test/admin"}]}]}"#).unwrap();
        std::fs::write(&second,"swagger: '2.0'\ninfo: {title: Pets}\nhost: example.test\npaths:\n  /pets/{id}:\n    get: {}\n").unwrap();
        let bundle = import_request_files(&[first, second]).unwrap();
        let original = Workspace::default();
        let workspace = build_import_workspace(&original, bundle).unwrap();
        assert!(original.collections.is_empty());
        assert_eq!(workspace.collections.len(), 2);
        let shop = &workspace.collections[0];
        assert_eq!(shop.name, "Shop");
        assert_eq!(shop.folders.len(), 2);
        assert_eq!(
            shop.folders[1].parent_folder_id.as_deref(),
            Some(shop.folders[0].id.as_str())
        );
        assert_eq!(
            shop.requests[0].folder_id.as_deref(),
            Some(shop.folders[0].id.as_str())
        );
        assert_eq!(
            workspace.collections[1].requests[0].definition.request.url,
            "https://example.test/pets/{{id}}"
        );
    }

    #[gpui::test]
    fn file_preview_and_save_persist_without_opening_or_changing_tabs(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
        store.initialize().unwrap();
        let mut app = None;
        let store_for_app = store.clone();
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            let bindings = shortcuts::capture_base_key_bindings(cx);
            crate::theme::configure(cx);
            let view = cx
                .new(|cx| ApiTester::new_with_database_store(bindings, store_for_app, window, cx));
            crate::register_app_action_handlers(&view, cx);
            app = Some(view.clone());
            Root::new(view, window, cx)
        });
        let app = app.unwrap();
        cx.simulate_resize(size(px(1200.), px(800.)));
        let file = directory.path().join("Users.yaml");
        std::fs::write(&file,"openapi: 3.0.0\ninfo: {title: Users}\nservers: [{url: 'https://example.test'}]\npaths:\n  /users:\n    get: {}\n").unwrap();
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.url.update(cx, |input, cx| {
                    input.set_value("https://keep.test/draft", window, cx)
                });
                let count = app.request_tabs.tabs().len();
                app.import_request_paths(&[file], window, cx);
                assert_eq!(app.request_tabs.tabs().len(), count);
                assert!(app.request_interchange.open);
            })
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("import-and-save-requests").is_some());
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let count = app.request_tabs.tabs().len();
                app.save_request_import(window, cx);
                assert_eq!(app.request_tabs.tabs().len(), count);
                assert_eq!(app.url.read(cx).value().as_ref(), "https://keep.test/draft");
                assert!(!app.request_interchange.open);
                assert_eq!(app.workspace.collections.last().unwrap().name, "Users");
            })
        });
        assert_eq!(
            store
                .load_workspace()
                .unwrap()
                .collections
                .last()
                .unwrap()
                .requests[0]
                .definition
                .request
                .url,
            "https://example.test/users"
        );
    }
}
