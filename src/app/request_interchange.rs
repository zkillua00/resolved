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
                    .placeholder(
                        "Paste cURL, Wget, PowerShell, OpenAPI, AsyncAPI, an IntelliJ HTTP file, or request code",
                    )
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
        if self.sending {
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
            match import_requests(&source) {
                Ok(bundle) => ImportPreview::Ready {
                    source_format: bundle.source_format,
                    request_count: bundle.requests.len(),
                },
                Err(error) => ImportPreview::Error(error.to_string()),
            }
        };
    }

    fn paste_request_import_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        if self.sending {
            return;
        }
        let source = self
            .request_interchange
            .import_editor
            .read(cx)
            .value(cx)
            .to_string();
        match import_requests(&source) {
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
        if self.sending || paths.is_empty() {
            return;
        }
        match import_request_files(paths) {
            Ok(bundle) => self.open_imported_requests(bundle, window, cx),
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
        let (status, can_import, import_label) = match &self.request_interchange.import_preview {
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
                            .label(import_label)
                            .small()
                            .primary()
                            .disabled(!can_import || self.sending)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.import_request_editor_source(window, cx);
                            })),
                    ),
            )
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
                Ok(bundle) => this.open_imported_requests(bundle, window, cx),
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

fn import_request_files(paths: &[PathBuf]) -> Result<ImportBundle, String> {
    let mut formats = Vec::new();
    let mut requests = Vec::new();
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
        let bundle =
            import_requests(&source).map_err(|error| format!("{}: {error}", path.display()))?;
        if !formats.contains(&bundle.source_format) {
            formats.push(bundle.source_format);
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
}
