use std::{collections::BTreeMap, time::Instant};

use super::*;
use crate::core::parse_json_lines;
use base64::Engine as _;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum WebSocketSection {
    Messages,
    #[default]
    Console,
    Replays,
    Automation,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum WebSocketConnectionStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum WebSocketTimelineDirection {
    Sent,
    Received,
    System,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum WebSocketTimelineFilter {
    #[default]
    All,
    Sent,
    Received,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum WebSocketLibrarySelection {
    Message(String),
    Template(String),
    NewTemplate,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum WebSocketQuickSendStage {
    #[default]
    Picker,
    Fill,
}

pub(in crate::app) struct WebSocketTimelineEntry {
    pub(in crate::app) id: u64,
    pub(in crate::app) direction: WebSocketTimelineDirection,
    pub(in crate::app) at: chrono::DateTime<Utc>,
    pub(in crate::app) kind: &'static str,
    pub(in crate::app) payload: String,
}

pub(in crate::app) struct WebSocketWorkspaceState {
    pub(in crate::app) document: WebSocketWorkspace,
    pub(in crate::app) url: Entity<InputState>,
    pub(in crate::app) headers: Entity<CodeEditor>,
    pub(in crate::app) composer: Entity<CodeEditor>,
    message_name: Entity<InputState>,
    pub(in crate::app) library_search: Entity<InputState>,
    library_selection: Option<WebSocketLibrarySelection>,
    pub(in crate::app) library_preview: Entity<CodeEditor>,
    pub(in crate::app) quick_send_query: Entity<InputState>,
    pub(in crate::app) quick_send_open: bool,
    pub(in crate::app) quick_send_stage: WebSocketQuickSendStage,
    quick_send_selected_template_id: Option<String>,
    quick_send_scroll: ScrollHandle,
    template_name: Entity<InputState>,
    pub(in crate::app) template_payload: Entity<CodeEditor>,
    replay_name: Entity<InputState>,
    pub(in crate::app) automation: Entity<CodeEditor>,
    section: WebSocketSection,
    status: WebSocketConnectionStatus,
    notice: Option<String>,
    pub(in crate::app) timeline: Vec<WebSocketTimelineEntry>,
    pub(in crate::app) timeline_filter: Entity<InputState>,
    timeline_direction_filter: WebSocketTimelineFilter,
    pub(in crate::app) timeline_scroll: ScrollHandle,
    pub(in crate::app) timeline_following: bool,
    selected_timeline_entry: Option<u64>,
    pub(in crate::app) timeline_preview: Entity<CodeEditor>,
    next_timeline_entry_id: u64,
    sent_session: Vec<(Instant, String)>,
    active_template_id: Option<String>,
    template_values: Vec<(String, Entity<InputState>)>,
    template_value_subscriptions: Vec<Subscription>,
    command_sender: Option<tokio::sync::mpsc::UnboundedSender<WebSocketCommand>>,
    abort_handle: Option<AbortHandle>,
    generation: u64,
    pub(in crate::app) mcp_connection_id: Option<u64>,
    mcp_last_event_id: u64,
    hydrating: bool,
}

impl WebSocketWorkspaceState {
    pub fn new(
        document: &WebSocketWorkspace,
        window: &mut Window,
        cx: &mut Context<ApiTester>,
    ) -> Self {
        let url = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("wss://echo.example.com/socket")
                .default_value(document.url.clone())
        });
        let headers_source =
            serde_json::to_string_pretty(&document.headers).unwrap_or_else(|_| "[]".to_owned());
        let headers = cx.new(|cx| {
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(CodeLanguage::Json)
                    .placeholder("Headers")
                    .rows(8)
                    .soft_wrap(false)
                    .format_action(true),
                window,
                cx,
            )
        });
        headers.update(cx, |editor, cx| {
            editor.set_value(headers_source, window, cx)
        });
        let composer = cx.new(|cx| {
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(code_language_for_raw_body(document.composer_language))
                    .placeholder("Message · {{variables}} supported")
                    .rows(10)
                    .soft_wrap(false)
                    .format_action(true),
                window,
                cx,
            )
        });
        composer.update(cx, |editor, cx| {
            editor.set_value(document.composer.clone(), window, cx)
        });
        let template_payload = cx.new(|cx| {
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(CodeLanguage::Json)
                    .placeholder("Template · use %{field}%")
                    .rows(8)
                    .soft_wrap(false)
                    .format_action(true),
                window,
                cx,
            )
        });
        let library_preview = cx.new(|cx| {
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(CodeLanguage::Plain)
                    .placeholder("Select a saved message")
                    .rows(12)
                    .soft_wrap(false)
                    .line_numbers(true)
                    .read_only(true)
                    .format_action(true),
                window,
                cx,
            )
        });
        let automation = cx.new(|cx| {
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(CodeLanguage::JavaScript)
                    .placeholder("if (ws.event.eventType === 'message') ws.send({ ack: true });")
                    .rows(16)
                    .soft_wrap(false)
                    .format_action(true),
                window,
                cx,
            )
        });
        automation.update(cx, |editor, cx| {
            editor.set_value(document.automation_source.clone(), window, cx)
        });
        let timeline_filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter messages"));
        let timeline_preview = cx.new(|cx| {
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(CodeLanguage::Plain)
                    .placeholder("Select a frame to inspect its payload")
                    .rows(10)
                    .soft_wrap(false)
                    .line_numbers(true)
                    .read_only(true)
                    .format_action(true),
                window,
                cx,
            )
        });
        Self {
            document: document.clone(),
            url,
            headers,
            composer,
            message_name: cx
                .new(|cx| InputState::new(window, cx).placeholder("Saved message name")),
            library_search: cx.new(|cx| InputState::new(window, cx).placeholder("Search messages")),
            library_selection: None,
            library_preview,
            quick_send_query: cx
                .new(|cx| InputState::new(window, cx).placeholder("Find a template")),
            quick_send_open: false,
            quick_send_stage: WebSocketQuickSendStage::Picker,
            quick_send_selected_template_id: None,
            quick_send_scroll: ScrollHandle::new(),
            template_name: cx.new(|cx| InputState::new(window, cx).placeholder("Template name")),
            template_payload,
            replay_name: cx.new(|cx| InputState::new(window, cx).placeholder("Replay name")),
            automation,
            section: WebSocketSection::Console,
            status: WebSocketConnectionStatus::Disconnected,
            notice: None,
            timeline: Vec::new(),
            timeline_filter,
            timeline_direction_filter: WebSocketTimelineFilter::All,
            timeline_scroll: ScrollHandle::new(),
            timeline_following: true,
            selected_timeline_entry: None,
            timeline_preview,
            next_timeline_entry_id: 0,
            sent_session: Vec::new(),
            active_template_id: None,
            template_values: Vec::new(),
            template_value_subscriptions: Vec::new(),
            command_sender: None,
            abort_handle: None,
            generation: 0,
            mcp_connection_id: None,
            mcp_last_event_id: 0,
            hydrating: false,
        }
    }
}

impl ApiTester {
    pub(super) fn sync_active_websocket_document(&mut self, cx: &mut Context<Self>) {
        if self.websocket_workspace.hydrating
            || !self.request_tabs.active().template().is_websocket()
        {
            return;
        }
        self.websocket_workspace.document.url =
            self.websocket_workspace.url.read(cx).value().to_string();
        self.websocket_workspace.document.composer = self
            .websocket_workspace
            .composer
            .read(cx)
            .value(cx)
            .to_string();
        self.websocket_workspace.document.automation_source = self
            .websocket_workspace
            .automation
            .read(cx)
            .value(cx)
            .to_string();
        if let Ok(headers) =
            parse_websocket_headers(self.websocket_workspace.headers.read(cx).value(cx).as_ref())
        {
            self.websocket_workspace.document.headers = headers;
        }
        self.snapshot_active_request_tab(cx);
        self.schedule_request_tabs_persist(cx);
    }

    pub(super) fn websocket_document(&mut self, cx: &App) -> Result<WebSocketWorkspace, String> {
        self.websocket_workspace.document.url =
            self.websocket_workspace.url.read(cx).value().to_string();
        self.websocket_workspace.document.headers =
            parse_websocket_headers(self.websocket_workspace.headers.read(cx).value(cx).as_ref())?;
        self.websocket_workspace.document.composer = self
            .websocket_workspace
            .composer
            .read(cx)
            .value(cx)
            .to_string();
        self.websocket_workspace.document.automation_source = self
            .websocket_workspace
            .automation
            .read(cx)
            .value(cx)
            .to_string();
        Ok(self.websocket_workspace.document.clone())
    }

    pub(super) fn load_websocket_document(
        &mut self,
        document: WebSocketWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.leave_websocket_view();
        self.websocket_workspace.hydrating = true;
        self.websocket_workspace.url.update(cx, |input, cx| {
            input.set_value(document.url.clone(), window, cx)
        });
        let headers =
            serde_json::to_string_pretty(&document.headers).unwrap_or_else(|_| "[]".to_owned());
        self.websocket_workspace
            .headers
            .update(cx, |editor, cx| editor.set_value(headers, window, cx));
        self.websocket_workspace.composer.update(cx, |editor, cx| {
            editor.set_language(code_language_for_raw_body(document.composer_language), cx);
            editor.set_value(document.composer.clone(), window, cx)
        });
        self.websocket_workspace
            .automation
            .update(cx, |editor, cx| {
                editor.set_value(document.automation_source.clone(), window, cx)
            });
        self.websocket_workspace.document = document;
        self.refresh_websocket_composer_inline_actions(cx);
        self.websocket_workspace.notice = None;
        self.websocket_workspace.timeline.clear();
        self.websocket_workspace
            .timeline_filter
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.websocket_workspace.timeline_direction_filter = WebSocketTimelineFilter::All;
        self.websocket_workspace.timeline_following = true;
        self.websocket_workspace.selected_timeline_entry = None;
        self.websocket_workspace.timeline_scroll.scroll_to_bottom();
        self.websocket_workspace.sent_session.clear();
        self.websocket_workspace.active_template_id = None;
        self.websocket_workspace.template_values.clear();
        self.websocket_workspace
            .template_value_subscriptions
            .clear();
        self.websocket_workspace.quick_send_open = false;
        self.websocket_workspace.quick_send_stage = WebSocketQuickSendStage::Picker;
        self.websocket_workspace.mcp_connection_id = None;
        self.websocket_workspace.mcp_last_event_id = 0;
        self.websocket_workspace.library_selection = None;
        self.websocket_workspace
            .library_search
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.websocket_workspace.hydrating = false;
    }

    fn select_websocket_composer_language(
        &mut self,
        language: RawBodyLanguage,
        cx: &mut Context<Self>,
    ) {
        self.websocket_workspace.document.composer_language = language;
        self.websocket_workspace.composer.update(cx, |editor, cx| {
            editor.set_language(code_language_for_raw_body(language), cx)
        });
        self.refresh_websocket_composer_inline_actions(cx);
        self.persist_websocket_document(cx);
        cx.notify();
    }

    pub(super) fn format_websocket_composer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let language = self.websocket_workspace.document.composer_language;
        let editor = self.websocket_workspace.composer.clone();
        self.format_websocket_editor(editor, language, "message", window, cx);
    }

    pub(super) fn format_websocket_timeline_preview(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = self.websocket_workspace.timeline_preview.clone();
        let language = match editor.read(cx).language() {
            CodeLanguage::Json => RawBodyLanguage::Json,
            CodeLanguage::JavaScript => RawBodyLanguage::JavaScript,
            CodeLanguage::TypeScript => RawBodyLanguage::TypeScript,
            _ => RawBodyLanguage::Text,
        };
        self.format_websocket_editor(editor, language, "frame payload", window, cx);
    }

    pub(super) fn format_websocket_library_preview(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = self.websocket_workspace.library_preview.clone();
        let language = match editor.read(cx).language() {
            CodeLanguage::Json => RawBodyLanguage::Json,
            CodeLanguage::JavaScript => RawBodyLanguage::JavaScript,
            CodeLanguage::TypeScript => RawBodyLanguage::TypeScript,
            _ => RawBodyLanguage::Text,
        };
        self.format_websocket_editor(editor, language, "saved message", window, cx);
    }

    pub(super) fn format_websocket_template(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = self.websocket_workspace.template_payload.clone();
        self.format_websocket_editor(editor, RawBodyLanguage::Json, "template", window, cx);
    }

    fn format_websocket_editor(
        &mut self,
        editor: Entity<CodeEditor>,
        language: RawBodyLanguage,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let source = editor.read(cx).value(cx).to_string();
        if source.trim().is_empty() {
            window.push_notification(
                Notification::warning(format!("The {label} buffer is empty.")),
                cx,
            );
            return;
        }
        let formatted = match format_raw_body_source(language, &source, &self.settings.formatter) {
            Ok(formatted) => formatted,
            Err(message) => {
                window.push_notification(Notification::error(message), cx);
                return;
            }
        };
        if formatted == source {
            window.push_notification(
                Notification::info(format!("The {label} is already formatted.")),
                cx,
            );
            return;
        }
        editor.update(cx, |editor, cx| editor.set_value(formatted, window, cx));
        window.push_notification(
            Notification::success(format!("Formatted {label} as {language}.")),
            cx,
        );
    }

    pub(super) fn stop_websocket(&mut self) {
        if let Some(sender) = self.websocket_workspace.command_sender.take() {
            let _ = sender.send(WebSocketCommand::Close);
        }
        if let Some(abort) = self.websocket_workspace.abort_handle.take() {
            abort.abort();
        }
        self.websocket_workspace.generation = self.websocket_workspace.generation.wrapping_add(1);
        self.websocket_workspace.status = WebSocketConnectionStatus::Disconnected;
    }

    pub(super) fn leave_websocket_view(&mut self) {
        if self.websocket_workspace.mcp_connection_id.is_some() {
            self.websocket_workspace.command_sender = None;
            self.websocket_workspace.mcp_connection_id = None;
            self.websocket_workspace.mcp_last_event_id = 0;
            self.websocket_workspace.status = WebSocketConnectionStatus::Disconnected;
            self.websocket_workspace.generation =
                self.websocket_workspace.generation.wrapping_add(1);
        } else {
            self.stop_websocket();
        }
    }

    fn connect_websocket(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.websocket_workspace.mcp_connection_id.is_some() {
            self.stop_mcp_websocket();
        } else {
            self.stop_websocket();
        }
        let document = match self.websocket_document(cx) {
            Ok(document) => document,
            Err(error) => {
                self.websocket_workspace.notice = Some(error);
                cx.notify();
                return;
            }
        };
        let connection_draft = RequestDraft {
            url: document.url.trim().to_owned(),
            headers: document.headers,
            ..RequestDraft::default()
        };
        let resolved = match resolve_request(&connection_draft, self.workspace.active_environment())
        {
            Ok(resolved) => resolved,
            Err(error) => {
                self.websocket_workspace.notice = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let url = resolved.request.url;
        if url.is_empty() {
            self.websocket_workspace.notice = Some("Enter a WebSocket URL first.".to_owned());
            cx.notify();
            return;
        }
        let headers = resolved.request.headers;
        let upstream_target = match self.workspace_providers.active_id() {
            WorkspaceProviderId::Local(_) => None,
            WorkspaceProviderId::Upstream { .. } => match self.active_upstream_workspace() {
                Ok(target) => Some(target),
                Err(error) => {
                    self.websocket_workspace.notice = Some(error);
                    cx.notify();
                    return;
                }
            },
        };
        let (command_sender, command_receiver) = tokio::sync::mpsc::unbounded_channel();
        let (signal_sender, mut signal_receiver) = tokio::sync::mpsc::unbounded_channel();
        let generation = self.websocket_workspace.generation;
        self.websocket_workspace.command_sender = Some(command_sender);
        self.websocket_workspace.status = WebSocketConnectionStatus::Connecting;
        self.websocket_workspace.notice = None;
        self.websocket_workspace.sent_session.clear();
        let vault = self.credential_vault.clone();
        let runtime = Arc::clone(&self.runtime);
        let upstream_client = self.upstream_execution_client.clone();
        let task = self.runtime.spawn(async move {
            let result = match upstream_target {
                None => {
                    run_websocket_connection(
                        &url,
                        &headers,
                        command_receiver,
                        signal_sender.clone(),
                    )
                    .await
                }
                Some(target) => {
                    let upstream_id = target.upstream_id.clone();
                    let credential = match runtime
                        .spawn_blocking(move || vault.load_upstream(&upstream_id))
                        .await
                    {
                        Ok(Ok(Some(credential))) if credential.expires_at > Utc::now() => {
                            credential
                        }
                        Ok(Ok(_)) => {
                            let _ = signal_sender.send(WebSocketSignal::Failed(
                                "Log in to this server again.".to_owned(),
                            ));
                            return;
                        }
                        Ok(Err(error)) => {
                            let _ = signal_sender.send(WebSocketSignal::Failed(error.to_string()));
                            return;
                        }
                        Err(error) => {
                            let _ = signal_sender.send(WebSocketSignal::Failed(error.to_string()));
                            return;
                        }
                    };
                    match get_upstream_execution_policy(
                        &upstream_client,
                        &target.base_url,
                        credential.bearer_token(),
                    )
                    .await
                    {
                        Ok(RequestExecutionMode::Local) => {
                            run_websocket_connection(
                                &url,
                                &headers,
                                command_receiver,
                                signal_sender.clone(),
                            )
                            .await
                        }
                        Ok(RequestExecutionMode::Server) => {
                            run_upstream_websocket_connection(
                                &target.base_url,
                                credential.bearer_token(),
                                &target.workspace_id,
                                &url,
                                &headers,
                                command_receiver,
                                signal_sender.clone(),
                            )
                            .await
                        }
                        Err(error) => {
                            let _ = signal_sender.send(WebSocketSignal::Failed(error.to_string()));
                            return;
                        }
                    }
                }
            };
            if let Err(error) = result {
                let _ = signal_sender.send(WebSocketSignal::Failed(error.to_string()));
            }
        });
        self.websocket_workspace.abort_handle = Some(task.abort_handle());
        cx.spawn_in(window, async move |this, cx| {
            while let Some(signal) = signal_receiver.recv().await {
                let _ = this.update_in(cx, |this, window, cx| {
                    if this.websocket_workspace.generation != generation {
                        return;
                    }
                    this.handle_websocket_signal(signal, window, cx);
                });
            }
        })
        .detach();
        cx.notify();
    }

    fn handle_websocket_signal(
        &mut self,
        signal: WebSocketSignal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match signal {
            WebSocketSignal::Connected => {
                self.websocket_workspace.status = WebSocketConnectionStatus::Connected;
                self.push_websocket_timeline(
                    WebSocketTimelineDirection::System,
                    "open",
                    "Connected".to_owned(),
                );
                self.run_websocket_automation(WebSocketAutomationEvent::opened(), window, cx);
            }
            WebSocketSignal::Text(payload) => {
                self.push_websocket_timeline(
                    WebSocketTimelineDirection::Received,
                    "text",
                    payload.clone(),
                );
                self.run_websocket_automation(WebSocketAutomationEvent::text(payload), window, cx);
            }
            WebSocketSignal::Binary(bytes) => {
                let preview = binary_preview(&bytes);
                self.push_websocket_timeline(
                    WebSocketTimelineDirection::Received,
                    "binary",
                    preview,
                );
                let event = WebSocketAutomationEvent {
                    event_type: "message".to_owned(),
                    data: None,
                    binary_base64: Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
                };
                self.run_websocket_automation(event, window, cx);
            }
            WebSocketSignal::Ping(bytes) => self.push_websocket_timeline(
                WebSocketTimelineDirection::Received,
                "ping",
                binary_preview(&bytes),
            ),
            WebSocketSignal::Pong(bytes) => self.push_websocket_timeline(
                WebSocketTimelineDirection::Received,
                "pong",
                binary_preview(&bytes),
            ),
            WebSocketSignal::Closed(reason) => {
                self.websocket_workspace.status = WebSocketConnectionStatus::Disconnected;
                self.websocket_workspace.command_sender = None;
                self.push_websocket_timeline(
                    WebSocketTimelineDirection::System,
                    "close",
                    reason.unwrap_or_else(|| "Connection closed".to_owned()),
                );
            }
            WebSocketSignal::Failed(error) => {
                self.websocket_workspace.status = WebSocketConnectionStatus::Disconnected;
                self.websocket_workspace.command_sender = None;
                self.websocket_workspace.notice = Some(error.clone());
                self.push_websocket_timeline(WebSocketTimelineDirection::System, "error", error);
            }
        }
        cx.notify();
    }

    fn run_websocket_automation(
        &mut self,
        event: WebSocketAutomationEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.websocket_workspace.document.automation_enabled {
            return;
        }
        let environment = self
            .workspace
            .active_environment()
            .map(|environment| {
                environment
                    .variables
                    .iter()
                    .filter(|variable| variable.enabled)
                    .map(|variable| (variable.key.clone(), variable.value.clone()))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        let source = self.websocket_workspace.document.automation_source.clone();
        let generation = self.websocket_workspace.generation;
        let task = self
            .runtime
            .spawn_blocking(move || execute_websocket_automation(&source, &event, &environment));
        cx.spawn_in(window, async move |this, cx| {
            let output = task.await;
            let _ = this.update_in(cx, |this, _, cx| {
                if this.websocket_workspace.generation != generation {
                    return;
                }
                match output {
                    Ok(Ok(output)) => {
                        for log in output.logs {
                            this.push_websocket_timeline(
                                WebSocketTimelineDirection::System,
                                "script",
                                log,
                            );
                        }
                        for payload in output.sends {
                            this.send_websocket_payload(payload, false, cx);
                        }
                    }
                    Ok(Err(error)) => this.push_websocket_timeline(
                        WebSocketTimelineDirection::System,
                        "script error",
                        error,
                    ),
                    Err(error) => this.push_websocket_timeline(
                        WebSocketTimelineDirection::System,
                        "script error",
                        error.to_string(),
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn push_websocket_timeline(
        &mut self,
        direction: WebSocketTimelineDirection,
        kind: &'static str,
        payload: String,
    ) {
        let should_follow = self.websocket_workspace.timeline.is_empty()
            || websocket_scroll_is_at_bottom(&self.websocket_workspace.timeline_scroll);
        self.websocket_workspace.timeline_following = should_follow;
        let id = self.websocket_workspace.next_timeline_entry_id;
        self.websocket_workspace.next_timeline_entry_id = id.wrapping_add(1);
        self.websocket_workspace
            .timeline
            .push(WebSocketTimelineEntry {
                id,
                direction,
                at: Utc::now(),
                kind,
                payload,
            });
        let excess = self
            .websocket_workspace
            .timeline
            .len()
            .saturating_sub(MAX_WEBSOCKET_TIMELINE_ENTRIES);
        if excess > 0 {
            self.websocket_workspace.timeline.drain(..excess);
        }
        if should_follow {
            self.websocket_workspace.timeline_scroll.scroll_to_bottom();
        }
    }

    pub(super) fn attach_mcp_websocket_ui(
        &mut self,
        connection_id: u64,
        status: &str,
        sender: tokio::sync::mpsc::UnboundedSender<WebSocketCommand>,
        events: &[super::control::ControlWebSocketEvent],
    ) {
        if self.websocket_workspace.mcp_connection_id == Some(connection_id) {
            self.websocket_workspace.command_sender = Some(sender);
            self.websocket_workspace.status = match status {
                "connecting" => WebSocketConnectionStatus::Connecting,
                "connected" | "closing" => WebSocketConnectionStatus::Connected,
                _ => WebSocketConnectionStatus::Disconnected,
            };
            self.websocket_workspace.section = WebSocketSection::Console;
            return;
        }
        self.websocket_workspace.mcp_connection_id = Some(connection_id);
        self.websocket_workspace.command_sender = Some(sender);
        self.websocket_workspace.status = match status {
            "connecting" => WebSocketConnectionStatus::Connecting,
            "connected" | "closing" => WebSocketConnectionStatus::Connected,
            _ => WebSocketConnectionStatus::Disconnected,
        };
        self.websocket_workspace.timeline.clear();
        self.websocket_workspace.mcp_last_event_id = 0;
        for event in events {
            self.mirror_mcp_websocket_event(connection_id, event);
        }
        self.websocket_workspace.section = WebSocketSection::Console;
        self.websocket_workspace.timeline_scroll.scroll_to_bottom();
    }

    pub(super) fn mirror_mcp_websocket_event(
        &mut self,
        connection_id: u64,
        event: &super::control::ControlWebSocketEvent,
    ) {
        if self.websocket_workspace.mcp_connection_id != Some(connection_id)
            || event.id <= self.websocket_workspace.mcp_last_event_id
        {
            return;
        }
        self.websocket_workspace.mcp_last_event_id = event.id;
        let direction = match event.direction {
            "sent" => WebSocketTimelineDirection::Sent,
            "received" => WebSocketTimelineDirection::Received,
            _ => WebSocketTimelineDirection::System,
        };
        let payload = event
            .payload
            .clone()
            .or_else(|| event.binary.as_deref().map(binary_preview))
            .unwrap_or_default();
        let kind = if event.kind == "script_error" {
            "script error"
        } else {
            event.kind
        };
        self.push_websocket_timeline(direction, kind, payload);
        match event.kind {
            "open" => self.websocket_workspace.status = WebSocketConnectionStatus::Connected,
            "close" | "error" => {
                self.websocket_workspace.status = WebSocketConnectionStatus::Disconnected;
                self.websocket_workspace.command_sender = None;
            }
            _ => {}
        }
    }

    pub(super) fn detach_mcp_websocket_ui(&mut self, connection_id: u64, reason: &str) {
        if self.websocket_workspace.mcp_connection_id != Some(connection_id) {
            return;
        }
        self.websocket_workspace.status = WebSocketConnectionStatus::Disconnected;
        self.websocket_workspace.command_sender = None;
        self.push_websocket_timeline(
            WebSocketTimelineDirection::System,
            "close",
            reason.to_owned(),
        );
        self.websocket_workspace.mcp_connection_id = None;
    }

    fn clear_websocket_timeline(&mut self) {
        self.websocket_workspace.timeline.clear();
        self.websocket_workspace.selected_timeline_entry = None;
        self.websocket_workspace.timeline_following = true;
        self.websocket_workspace.timeline_scroll.scroll_to_bottom();
    }

    fn resolve_websocket_text(&self, text: &str) -> Result<String, String> {
        let draft = RequestDraft {
            url: text.to_owned(),
            ..RequestDraft::default()
        };
        let resolved = resolve_request(&draft, self.workspace.active_environment())
            .map_err(|error| error.to_string())?;
        Ok(resolved.request.url)
    }

    fn send_websocket_payload(
        &mut self,
        payload: String,
        _reset_composer: bool,
        cx: &mut Context<Self>,
    ) {
        let payload = match self.resolve_websocket_text(&payload) {
            Ok(payload) => payload,
            Err(error) => {
                self.websocket_workspace.notice = Some(error);
                cx.notify();
                return;
            }
        };
        self.send_resolved_websocket_payload(payload, cx);
    }

    fn send_resolved_websocket_payload(&mut self, payload: String, cx: &mut Context<Self>) {
        let Some(sender) = self.websocket_workspace.command_sender.as_ref() else {
            self.websocket_workspace.notice = Some("Connect before sending a message.".to_owned());
            cx.notify();
            return;
        };
        if sender
            .send(WebSocketCommand::SendText(payload.clone()))
            .is_err()
        {
            self.websocket_workspace.notice =
                Some("The WebSocket connection is no longer available.".to_owned());
            cx.notify();
            return;
        }
        self.websocket_workspace
            .sent_session
            .push((Instant::now(), payload.clone()));
        self.push_websocket_timeline(WebSocketTimelineDirection::Sent, "text", payload.clone());
        self.record_mcp_websocket_ui_text(payload);
        self.websocket_workspace.notice = None;
        cx.notify();
    }

    fn resolve_websocket_json_lines(&self, source: &str) -> Result<Vec<String>, String> {
        let resolved = self.resolve_websocket_text(source)?;
        let records = parse_json_lines(&resolved)?;
        if records.is_empty() {
            return Err("The JSONL composer has no messages to send.".to_owned());
        }
        Ok(records
            .into_iter()
            .map(|record| resolved[record.range].to_owned())
            .collect())
    }

    fn send_websocket_json_lines(&mut self, source: &str, cx: &mut Context<Self>) -> bool {
        let payloads = match self.resolve_websocket_json_lines(source) {
            Ok(payloads) => payloads,
            Err(error) => {
                self.websocket_workspace.notice = Some(error);
                cx.notify();
                return false;
            }
        };
        if self.websocket_workspace.command_sender.is_none() {
            self.websocket_workspace.notice = Some("Connect before sending a message.".to_owned());
            cx.notify();
            return false;
        }
        for payload in payloads {
            self.send_resolved_websocket_payload(payload, cx);
            if self.websocket_workspace.notice.is_some() {
                return false;
            }
        }
        true
    }

    pub(super) fn refresh_websocket_composer_inline_actions(&mut self, cx: &mut Context<Self>) {
        let actions =
            if self.websocket_workspace.document.composer_language == RawBodyLanguage::JsonLines {
                let source = self.websocket_workspace.composer.read(cx).value(cx);
                parse_json_lines(&source)
                    .unwrap_or_default()
                    .into_iter()
                    .enumerate()
                    .map(|(id, record)| InputInlineAction {
                        id,
                        row: match self.settings.editor.inline_action_placement {
                            EditorInlineActionPlacement::Above => record.start_line - 1,
                            EditorInlineActionPlacement::After => source[..record.range.end]
                                .bytes()
                                .filter(|byte| *byte == b'\n')
                                .count(),
                        },
                        label: "▷ Send".into(),
                        placement: match self.settings.editor.inline_action_placement {
                            EditorInlineActionPlacement::Above => InputInlineActionPlacement::Above,
                            EditorInlineActionPlacement::After => InputInlineActionPlacement::After,
                        },
                    })
                    .collect()
            } else {
                Vec::new()
            };
        self.websocket_workspace
            .composer
            .update(cx, |editor, cx| editor.set_inline_actions(actions, cx));
    }

    pub(super) fn send_websocket_json_record(&mut self, id: usize, cx: &mut Context<Self>) {
        let source = self.websocket_workspace.composer.read(cx).value(cx);
        let resolved = match self.resolve_websocket_text(&source) {
            Ok(payload) => payload,
            Err(error) => {
                self.websocket_workspace.notice = Some(error);
                cx.notify();
                return;
            }
        };
        let records = match parse_json_lines(&resolved) {
            Ok(records) => records,
            Err(error) => {
                self.websocket_workspace.notice = Some(error);
                cx.notify();
                return;
            }
        };
        let Some(record) = records.get(id) else {
            self.websocket_workspace.notice =
                Some("That JSONL record changed before it could be sent.".to_owned());
            cx.notify();
            return;
        };
        self.send_resolved_websocket_payload(resolved[record.range.clone()].to_owned(), cx);
    }

    fn send_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let payload = self
            .websocket_workspace
            .composer
            .read(cx)
            .value(cx)
            .to_string();
        if payload.is_empty() {
            return;
        }
        let sent =
            if self.websocket_workspace.document.composer_language == RawBodyLanguage::JsonLines {
                self.send_websocket_json_lines(&payload, cx)
            } else {
                self.send_websocket_payload(payload, false, cx);
                self.websocket_workspace.notice.is_none()
            };
        if self.websocket_workspace.document.reset_input_after_send && sent {
            self.websocket_workspace
                .composer
                .update(cx, |editor, cx| editor.set_value("", window, cx));
        }
    }

    fn save_websocket_message(&mut self, cx: &mut Context<Self>) {
        let name = self
            .websocket_workspace
            .message_name
            .read(cx)
            .value()
            .trim()
            .to_owned();
        let payload = self
            .websocket_workspace
            .composer
            .read(cx)
            .value(cx)
            .to_string();
        if name.is_empty() || payload.is_empty() {
            self.websocket_workspace.notice =
                Some("A saved message needs a name and payload.".to_owned());
            cx.notify();
            return;
        }
        self.websocket_workspace
            .document
            .messages
            .push(WebSocketSavedMessage {
                id: new_websocket_id("message"),
                name,
                payload,
                language: self.websocket_workspace.document.composer_language,
            });
        self.persist_websocket_document(cx);
    }

    fn save_websocket_template(&mut self, cx: &mut Context<Self>) {
        let name = self
            .websocket_workspace
            .template_name
            .read(cx)
            .value()
            .trim()
            .to_owned();
        let payload = self
            .websocket_workspace
            .template_payload
            .read(cx)
            .value(cx)
            .to_string();
        if name.is_empty() || payload.is_empty() {
            self.websocket_workspace.notice =
                Some("A template needs a name and payload.".to_owned());
            cx.notify();
            return;
        }
        if let Err(error) = template_variable_names(&payload) {
            self.websocket_workspace.notice = Some(error.to_string());
            cx.notify();
            return;
        }
        let selected_id = match &self.websocket_workspace.library_selection {
            Some(WebSocketLibrarySelection::Template(id)) => Some(id.clone()),
            _ => None,
        };
        if let Some(template) = selected_id.as_deref().and_then(|id| {
            self.websocket_workspace
                .document
                .templates
                .iter_mut()
                .find(|template| template.id == id)
        }) {
            template.name = name;
            template.payload = payload;
        } else {
            let id = new_websocket_id("template");
            self.websocket_workspace
                .document
                .templates
                .push(WebSocketMessageTemplate {
                    id: id.clone(),
                    name,
                    payload,
                });
            self.websocket_workspace.library_selection =
                Some(WebSocketLibrarySelection::Template(id));
        }
        self.persist_websocket_document(cx);
    }

    fn new_websocket_template(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.websocket_workspace.library_selection = Some(WebSocketLibrarySelection::NewTemplate);
        self.websocket_workspace.active_template_id = None;
        self.websocket_workspace.template_values.clear();
        self.websocket_workspace
            .template_value_subscriptions
            .clear();
        self.websocket_workspace
            .template_name
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.websocket_workspace
            .template_payload
            .update(cx, |editor, cx| editor.set_value("", window, cx));
        cx.notify();
    }

    fn select_websocket_library_message(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(message) = self
            .websocket_workspace
            .document
            .messages
            .iter()
            .find(|message| message.id == id)
        else {
            return;
        };
        let payload = message.payload.clone();
        let language = message.language;
        self.websocket_workspace.library_selection = Some(WebSocketLibrarySelection::Message(id));
        self.websocket_workspace.active_template_id = None;
        self.websocket_workspace.template_values.clear();
        self.websocket_workspace
            .template_value_subscriptions
            .clear();
        self.websocket_workspace
            .library_preview
            .update(cx, |editor, cx| {
                editor.set_language(code_language_for_raw_body(language), cx);
                editor.set_value(payload, window, cx);
            });
        cx.notify();
    }

    fn select_websocket_library_template(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(template) = self
            .websocket_workspace
            .document
            .templates
            .iter()
            .find(|template| template.id == id)
        else {
            return;
        };
        let name = template.name.clone();
        let payload = template.payload.clone();
        self.websocket_workspace.library_selection = Some(WebSocketLibrarySelection::Template(id));
        self.websocket_workspace.active_template_id = None;
        self.websocket_workspace.template_values.clear();
        self.websocket_workspace
            .template_value_subscriptions
            .clear();
        self.websocket_workspace
            .template_name
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.websocket_workspace
            .template_payload
            .update(cx, |editor, cx| editor.set_value(payload, window, cx));
        cx.notify();
    }

    fn delete_selected_websocket_library_item(&mut self, cx: &mut Context<Self>) {
        match self.websocket_workspace.library_selection.take() {
            Some(WebSocketLibrarySelection::Message(id)) => self
                .websocket_workspace
                .document
                .messages
                .retain(|message| message.id != id),
            Some(WebSocketLibrarySelection::Template(id)) => self
                .websocket_workspace
                .document
                .templates
                .retain(|template| template.id != id),
            Some(WebSocketLibrarySelection::NewTemplate) | None => return,
        }
        self.websocket_workspace.active_template_id = None;
        self.websocket_workspace.template_values.clear();
        self.websocket_workspace
            .template_value_subscriptions
            .clear();
        self.persist_websocket_document(cx);
    }

    fn load_saved_websocket_message(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(message) = self
            .websocket_workspace
            .document
            .messages
            .iter()
            .find(|message| message.id == id)
        else {
            return;
        };
        let payload = message.payload.clone();
        let language = message.language;
        self.websocket_workspace.document.composer_language = language;
        self.websocket_workspace.composer.update(cx, |editor, cx| {
            editor.set_language(code_language_for_raw_body(language), cx);
            editor.set_value(payload, window, cx);
        });
        self.refresh_websocket_composer_inline_actions(cx);
        self.persist_websocket_document(cx);
        self.websocket_workspace.section = WebSocketSection::Console;
        cx.notify();
    }

    fn select_websocket_template(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(template) = self
            .websocket_workspace
            .document
            .templates
            .iter()
            .find(|item| item.id == id)
        else {
            return;
        };
        let names = match template_variable_names(&template.payload) {
            Ok(names) => names,
            Err(error) => {
                self.websocket_workspace.notice = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        self.websocket_workspace.active_template_id = Some(id);
        self.websocket_workspace
            .template_value_subscriptions
            .clear();
        self.websocket_workspace.template_values = names
            .into_iter()
            .map(|name| {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder(name.clone()));
                (name, input)
            })
            .collect();
        let fields = self
            .websocket_workspace
            .template_values
            .iter()
            .map(|(name, input)| (name.clone(), input.clone()))
            .collect::<Vec<_>>();
        self.websocket_workspace.template_value_subscriptions = fields
            .into_iter()
            .map(|(name, input)| {
                cx.subscribe_in(&input, window, move |this, _, event, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                    if matches!(event, InputEvent::PressEnter { secondary: false }) {
                        this.focus_next_websocket_template_value(&name, window, cx);
                    }
                })
            })
            .collect();
        cx.notify();
    }

    fn send_active_websocket_template(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.websocket_workspace.active_template_id.as_deref() else {
            return false;
        };
        let Some(template) = self
            .websocket_workspace
            .document
            .templates
            .iter()
            .find(|item| item.id == id)
        else {
            return false;
        };
        let values = self
            .websocket_workspace
            .template_values
            .iter()
            .map(|(name, input)| (name.clone(), input.read(cx).value().to_string()))
            .collect::<BTreeMap<_, _>>();
        match render_message_template(&template.payload, &values) {
            Ok(payload) => {
                self.send_websocket_payload(payload, false, cx);
                self.websocket_workspace.notice.is_none()
            }
            Err(error) => {
                self.websocket_workspace.notice = Some(error.to_string());
                cx.notify();
                false
            }
        }
    }

    fn focus_next_websocket_template_value(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .websocket_workspace
            .template_values
            .iter()
            .position(|(field, _)| field == name)
        else {
            return;
        };
        if let Some((_, input)) = self.websocket_workspace.template_values.get(index + 1) {
            input.read(cx).focus_handle(cx).focus(window);
        }
    }

    pub(super) fn open_websocket_quick_send(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.request_tabs.active().template().is_websocket() {
            return;
        }
        if self.websocket_workspace.document.templates.is_empty() {
            self.websocket_workspace.section = WebSocketSection::Messages;
            self.websocket_workspace.notice =
                Some("Create a template to use Quick send.".to_owned());
            cx.notify();
            return;
        }
        self.websocket_workspace.quick_send_open = true;
        self.websocket_workspace.quick_send_stage = WebSocketQuickSendStage::Picker;
        self.websocket_workspace.quick_send_selected_template_id = self
            .websocket_workspace
            .document
            .templates
            .first()
            .map(|template| template.id.clone());
        self.websocket_workspace.active_template_id = None;
        self.websocket_workspace.template_values.clear();
        self.websocket_workspace
            .template_value_subscriptions
            .clear();
        self.websocket_workspace
            .quick_send_query
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.websocket_workspace
            .quick_send_query
            .read(cx)
            .focus_handle(cx)
            .focus(window);
        cx.notify();
    }

    fn open_websocket_quick_send_for(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.websocket_workspace.quick_send_open = true;
        self.websocket_workspace.quick_send_stage = WebSocketQuickSendStage::Fill;
        self.websocket_workspace.quick_send_selected_template_id = Some(id.clone());
        self.select_websocket_template(id, window, cx);
        if let Some((_, input)) = self.websocket_workspace.template_values.first() {
            input.read(cx).focus_handle(cx).focus(window);
        }
        cx.notify();
    }

    fn websocket_quick_send_matches(&self, cx: &App) -> Vec<String> {
        let query = self
            .websocket_workspace
            .quick_send_query
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        self.websocket_workspace
            .document
            .templates
            .iter()
            .filter(|template| {
                websocket_library_item_matches(&template.name, &template.payload, &query)
            })
            .map(|template| template.id.clone())
            .collect()
    }

    pub(super) fn reset_websocket_quick_send_selection(&mut self, cx: &mut Context<Self>) {
        self.websocket_workspace.quick_send_selected_template_id =
            self.websocket_quick_send_matches(cx).into_iter().next();
        self.websocket_workspace.quick_send_scroll.scroll_to_item(0);
        cx.notify();
    }

    pub(super) fn move_websocket_quick_send_selection(
        &mut self,
        direction: isize,
        cx: &mut Context<Self>,
    ) {
        let matches = self.websocket_quick_send_matches(cx);
        if matches.is_empty() {
            self.websocket_workspace.quick_send_selected_template_id = None;
            cx.notify();
            return;
        }
        let current = self
            .websocket_workspace
            .quick_send_selected_template_id
            .as_ref()
            .and_then(|selected| matches.iter().position(|id| id == selected))
            .unwrap_or(0);
        let selected = current
            .saturating_add_signed(direction)
            .min(matches.len() - 1);
        self.websocket_workspace.quick_send_selected_template_id = Some(matches[selected].clone());
        self.websocket_workspace
            .quick_send_scroll
            .scroll_to_item(selected);
        cx.notify();
    }

    pub(super) fn select_websocket_quick_send_template(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let matches = self.websocket_quick_send_matches(cx);
        let id = self
            .websocket_workspace
            .quick_send_selected_template_id
            .clone()
            .filter(|selected| matches.iter().any(|id| id == selected))
            .or_else(|| matches.into_iter().next());
        if let Some(id) = id {
            self.open_websocket_quick_send_for(id, window, cx);
        }
    }

    pub(super) fn close_websocket_quick_send(&mut self, cx: &mut Context<Self>) {
        self.websocket_workspace.quick_send_open = false;
        self.websocket_workspace.quick_send_stage = WebSocketQuickSendStage::Picker;
        self.websocket_workspace.quick_send_selected_template_id = None;
        self.websocket_workspace.active_template_id = None;
        self.websocket_workspace.template_values.clear();
        self.websocket_workspace
            .template_value_subscriptions
            .clear();
        cx.notify();
    }

    fn back_websocket_quick_send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.websocket_workspace.quick_send_stage = WebSocketQuickSendStage::Picker;
        self.websocket_workspace.active_template_id = None;
        self.websocket_workspace.template_values.clear();
        self.websocket_workspace
            .template_value_subscriptions
            .clear();
        self.websocket_workspace
            .quick_send_query
            .read(cx)
            .focus_handle(cx)
            .focus(window);
        cx.notify();
    }

    pub(super) fn submit_websocket_quick_send(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.websocket_workspace.quick_send_open {
            return false;
        }
        if self.websocket_workspace.quick_send_stage == WebSocketQuickSendStage::Picker {
            return false;
        }
        if self.send_active_websocket_template(cx) {
            self.close_websocket_quick_send(cx);
        }
        true
    }

    pub(super) fn handle_websocket_quick_send_submit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.websocket_workspace.quick_send_open {
            return false;
        }
        if self.websocket_workspace.quick_send_stage == WebSocketQuickSendStage::Picker {
            self.select_websocket_quick_send_template(window, cx);
        } else {
            self.submit_websocket_quick_send(cx);
        }
        true
    }

    fn save_websocket_replay(&mut self, cx: &mut Context<Self>) {
        let name = self
            .websocket_workspace
            .replay_name
            .read(cx)
            .value()
            .trim()
            .to_owned();
        let frames = replay_frames(&self.websocket_workspace.sent_session);
        if name.is_empty() || frames.is_empty() {
            self.websocket_workspace.notice =
                Some("A replay needs a name and at least one sent message.".to_owned());
            cx.notify();
            return;
        }
        self.websocket_workspace
            .document
            .replays
            .push(WebSocketReplay {
                id: new_websocket_id("replay"),
                name,
                frames,
            });
        self.persist_websocket_document(cx);
    }

    fn play_websocket_replay(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(sender) = self.websocket_workspace.command_sender.clone() else {
            self.websocket_workspace.notice =
                Some("Connect before replaying a session.".to_owned());
            cx.notify();
            return;
        };
        let Some(replay) = self
            .websocket_workspace
            .document
            .replays
            .iter()
            .find(|item| item.id == id)
        else {
            return;
        };
        let frames = replay.frames.clone();
        for frame in &frames {
            self.push_websocket_timeline(
                WebSocketTimelineDirection::Sent,
                "replay",
                frame.payload.clone(),
            );
        }
        self.runtime.spawn(replay_websocket_frames(frames, sender));
        cx.notify();
    }

    fn persist_websocket_document(&mut self, cx: &mut Context<Self>) {
        if self.websocket_workspace.hydrating {
            return;
        }
        self.websocket_workspace.notice = None;
        self.snapshot_active_request_tab(cx);
        self.schedule_request_tabs_persist(cx);
        cx.notify();
    }

    pub(super) fn render_websocket_workspace(&self, cx: &mut Context<Self>) -> AnyElement {
        let connected = self.websocket_workspace.status == WebSocketConnectionStatus::Connected;
        let status = match self.websocket_workspace.status {
            WebSocketConnectionStatus::Disconnected => "Disconnected",
            WebSocketConnectionStatus::Connecting => "Connecting…",
            WebSocketConnectionStatus::Connected => "Connected",
        };
        v_flex()
            .relative()
            .debug_selector(|| "websocket-workspace".to_owned())
            .size_full()
            .min_h_0()
            .bg(cx.api_surface())
            .child(
                h_flex()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(cx.api_outline_variant())
                    .bg(cx.api_surface_low())
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child("WS"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.websocket_workspace.url)),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .px_3()
                            .py_1()
                            .rounded_full()
                            .bg(if connected {
                                cx.theme().success.opacity(0.12)
                            } else {
                                cx.api_surface()
                            })
                            .child(div().size(px(7.)).rounded_full().bg(if connected {
                                cx.theme().success
                            } else {
                                cx.theme().muted_foreground
                            }))
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(if connected {
                                        cx.theme().success
                                    } else {
                                        cx.theme().muted_foreground
                                    })
                                    .child(status),
                            ),
                    )
                    .when(
                        self.websocket_workspace.mcp_connection_id.is_some(),
                        |this| {
                            this.child(
                                div()
                                    .px_3()
                                    .py_1()
                                    .rounded_full()
                                    .bg(cx.theme().info.opacity(0.12))
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().info)
                                    .child(if connected {
                                        "MCP controlled"
                                    } else {
                                        "MCP session"
                                    }),
                            )
                        },
                    )
                    .child(
                        Button::new("websocket-connect")
                            .label(if connected { "Disconnect" } else { "Connect" })
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.websocket_workspace.status
                                    == WebSocketConnectionStatus::Connected
                                {
                                    this.stop_websocket();
                                    cx.notify();
                                } else {
                                    this.connect_websocket(window, cx);
                                }
                            })),
                    ),
            )
            .child(
                div().px_4().pt_1().child(
                    TabBar::new("websocket-sections")
                        .underline()
                        .children(["Console", "Messages", "Replays", "Automation"])
                        .selected_index(match self.websocket_workspace.section {
                            WebSocketSection::Console => 0,
                            WebSocketSection::Messages => 1,
                            WebSocketSection::Replays => 2,
                            WebSocketSection::Automation => 3,
                        })
                        .on_click(cx.listener(|this, index: &usize, _, cx| {
                            this.websocket_workspace.section = match index {
                                1 => WebSocketSection::Messages,
                                2 => WebSocketSection::Replays,
                                3 => WebSocketSection::Automation,
                                _ => WebSocketSection::Console,
                            };
                            cx.notify();
                        })),
                ),
            )
            .child(div().flex_1().min_h_0().overflow_hidden().child(
                match self.websocket_workspace.section {
                    WebSocketSection::Console => self.render_websocket_console(cx),
                    WebSocketSection::Messages => self.render_websocket_messages(cx),
                    WebSocketSection::Replays => self.render_websocket_replays(cx),
                    WebSocketSection::Automation => self.render_websocket_automation(cx),
                },
            ))
            .when_some(self.websocket_workspace.notice.clone(), |this, notice| {
                this.child(
                    h_flex()
                        .absolute()
                        .top(px(62.))
                        .left(px(16.))
                        .right(px(16.))
                        .min_w_0()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .border_1()
                        .border_color(cx.theme().danger.opacity(0.25))
                        .bg(cx.api_surface())
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .shadow_md()
                        .child(div().flex_1().min_w_0().whitespace_normal().child(notice))
                        .child(
                            Button::new("dismiss-websocket-notice")
                                .icon(IconName::Close)
                                .xsmall()
                                .ghost()
                                .tooltip("Dismiss")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.websocket_workspace.notice = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when(self.websocket_workspace.quick_send_open, |this| {
                this.child(deferred(self.render_websocket_quick_send(cx)).with_priority(2))
            })
            .into_any_element()
    }

    fn render_websocket_quick_send(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .flex()
            .items_center()
            .justify_center()
            .p_6()
            .bg(cx.theme().background.opacity(0.72))
            .child(
                v_flex()
                    .debug_selector(|| "websocket-quick-send".to_owned())
                    .w_full()
                    .max_w(px(620.))
                    .max_h(px(560.))
                    .overflow_hidden()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.api_outline_variant())
                    .bg(cx.api_surface())
                    .shadow_lg()
                    .occlude()
                    .child(match self.websocket_workspace.quick_send_stage {
                        WebSocketQuickSendStage::Picker => {
                            self.render_websocket_quick_send_picker(cx)
                        }
                        WebSocketQuickSendStage::Fill => self.render_websocket_quick_send_form(cx),
                    }),
            )
            .into_any_element()
    }

    fn render_websocket_quick_send_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let query = self
            .websocket_workspace
            .quick_send_query
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        let templates = self
            .websocket_workspace
            .document
            .templates
            .iter()
            .filter(|template| {
                websocket_library_item_matches(&template.name, &template.payload, &query)
            })
            .collect::<Vec<_>>();
        let rows = templates
            .iter()
            .map(|template| {
                let id = template.id.clone();
                let selected = self
                    .websocket_workspace
                    .quick_send_selected_template_id
                    .as_deref()
                    == Some(template.id.as_str());
                div()
                    .id(SharedString::from(format!(
                        "quick-send-template-{}",
                        template.id
                    )))
                    .w_full()
                    .cursor_pointer()
                    .rounded_md()
                    .when(selected, |this| {
                        this.bg(cx.theme().sidebar_accent.opacity(0.82))
                    })
                    .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.62)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_websocket_quick_send_for(id.clone(), window, cx);
                    }))
                    .child(
                        v_flex()
                            .gap_1()
                            .px_3()
                            .py_2()
                            .child(div().text_sm().font_semibold().child(template.name.clone()))
                            .child(
                                div()
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(compact_label(&template.payload, 72)),
                            ),
                    )
            })
            .collect::<Vec<_>>();
        v_flex()
            .min_h_0()
            .child(
                h_flex()
                    .h(px(54.))
                    .flex_shrink_0()
                    .px_4()
                    .border_b_1()
                    .border_color(cx.api_outline_variant())
                    .child(div().flex_1().font_semibold().child("Quick send template"))
                    .child(
                        Button::new("close-websocket-quick-send")
                            .icon(IconName::Close)
                            .xsmall()
                            .ghost()
                            .tooltip("Close")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_websocket_quick_send(cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .p_3()
                    .border_b_1()
                    .border_color(cx.api_outline_variant())
                    .child(
                        Input::new(&self.websocket_workspace.quick_send_query)
                            .prefix(IconName::Search)
                            .cleanable(true),
                    ),
            )
            .child(
                v_flex()
                    .id("websocket-quick-send-scroll")
                    .flex_1()
                    .min_h_0()
                    .max_h(px(380.))
                    .track_scroll(&self.websocket_workspace.quick_send_scroll)
                    .overflow_y_scrollbar()
                    .gap_1()
                    .p_2()
                    .children(rows)
                    .when(templates.is_empty(), |this| {
                        this.child(
                            div()
                                .px_4()
                                .py_10()
                                .text_center()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("No matching templates"),
                        )
                    }),
            )
            .child(
                h_flex()
                    .h(px(42.))
                    .flex_shrink_0()
                    .px_4()
                    .border_t_1()
                    .border_color(cx.api_outline_variant())
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("↑↓ Navigate  ·  Enter select  ·  Esc close"),
            )
            .into_any_element()
    }

    fn render_websocket_quick_send_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let template = self
            .websocket_workspace
            .active_template_id
            .as_deref()
            .and_then(|id| {
                self.websocket_workspace
                    .document
                    .templates
                    .iter()
                    .find(|template| template.id == id)
            });
        let Some(template) = template else {
            return self.render_websocket_quick_send_picker(cx);
        };
        let values = self
            .websocket_workspace
            .template_values
            .iter()
            .map(|(name, input)| (name.clone(), input.read(cx).value().to_string()))
            .collect::<BTreeMap<_, _>>();
        let preview = render_message_template(&template.payload, &values)
            .unwrap_or_else(|_| template.payload.clone());
        let connected = self.websocket_workspace.status == WebSocketConnectionStatus::Connected;
        v_flex()
            .min_h_0()
            .child(
                h_flex()
                    .h(px(54.))
                    .flex_shrink_0()
                    .gap_2()
                    .px_4()
                    .border_b_1()
                    .border_color(cx.api_outline_variant())
                    .child(
                        Button::new("back-websocket-quick-send")
                            .label("Back")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.back_websocket_quick_send(window, cx);
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_semibold()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .child(template.name.clone()),
                    )
                    .child(
                        Button::new("close-websocket-quick-send-form")
                            .icon(IconName::Close)
                            .xsmall()
                            .ghost()
                            .tooltip("Close")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_websocket_quick_send(cx);
                            })),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .max_h(px(420.))
                    .overflow_y_scrollbar()
                    .gap_5()
                    .p_5()
                    .when(
                        self.websocket_workspace.template_values.is_empty(),
                        |this| {
                            this.child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("This template has no prompted values."),
                            )
                        },
                    )
                    .children(self.websocket_workspace.template_values.iter().map(
                        |(name, input)| {
                            v_flex()
                                .gap_2()
                                .child(div().text_sm().font_semibold().child(name.clone()))
                                .child(Input::new(input))
                        },
                    ))
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("PREVIEW"),
                            )
                            .child(
                                div()
                                    .max_h(px(120.))
                                    .rounded_md()
                                    .border_1()
                                    .border_color(cx.api_outline_variant())
                                    .bg(cx.api_surface_low())
                                    .px_3()
                                    .py_2()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_xs()
                                    .whitespace_normal()
                                    .overflow_y_scrollbar()
                                    .child(preview),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .debug_selector(|| "websocket-quick-send-footer".to_owned())
                    .min_h(px(58.))
                    .flex_shrink_0()
                    .gap_3()
                    .px_5()
                    .py_3()
                    .border_t_1()
                    .border_color(cx.api_outline_variant())
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(if connected {
                                cx.theme().success
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(if connected {
                                if cfg!(target_os = "macos") {
                                    "Connected · ⌘↩ to send"
                                } else {
                                    "Connected · Ctrl+Enter to send"
                                }
                            } else {
                                "Connect before sending"
                            }),
                    )
                    .child(
                        Button::new("send-websocket-quick-template")
                            .label("Send")
                            .small()
                            .primary()
                            .disabled(!connected)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.submit_websocket_quick_send(cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_websocket_messages(&self, cx: &mut Context<Self>) -> AnyElement {
        let query = self
            .websocket_workspace
            .library_search
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        let selected = self.websocket_workspace.library_selection.clone();
        let mut rows = Vec::new();
        for message in &self.websocket_workspace.document.messages {
            if !websocket_library_item_matches(&message.name, &message.payload, &query) {
                continue;
            }
            let id = message.id.clone();
            let is_selected = selected.as_ref().is_some_and(|selection| {
                selection == &WebSocketLibrarySelection::Message(id.clone())
            });
            rows.push(
                div()
                    .id(SharedString::from(format!(
                        "websocket-message-row-{}",
                        message.id
                    )))
                    .w_full()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_websocket_library_message(id.clone(), window, cx);
                    }))
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .px_2()
                            .py_2()
                            .rounded_md()
                            .when(is_selected, |this| this.bg(cx.theme().sidebar_accent))
                            .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.62)))
                            .child(
                                div()
                                    .w(px(22.))
                                    .h(px(22.))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_md()
                                    .bg(cx.theme().info.opacity(0.14))
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().info)
                                    .child("M"),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .whitespace_nowrap()
                                            .overflow_hidden()
                                            .text_sm()
                                            .font_semibold()
                                            .child(message.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .whitespace_nowrap()
                                            .overflow_hidden()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(compact_label(&message.payload, 42)),
                                    ),
                            ),
                    )
                    .into_any_element(),
            );
        }
        for template in &self.websocket_workspace.document.templates {
            if !websocket_library_item_matches(&template.name, &template.payload, &query) {
                continue;
            }
            let id = template.id.clone();
            let is_selected = selected.as_ref().is_some_and(|selection| {
                selection == &WebSocketLibrarySelection::Template(id.clone())
            });
            rows.push(
                div()
                    .id(SharedString::from(format!(
                        "websocket-template-row-{}",
                        template.id
                    )))
                    .w_full()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_websocket_library_template(id.clone(), window, cx);
                    }))
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .px_2()
                            .py_2()
                            .rounded_md()
                            .when(is_selected, |this| this.bg(cx.theme().sidebar_accent))
                            .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.62)))
                            .child(
                                div()
                                    .w(px(22.))
                                    .h(px(22.))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_md()
                                    .bg(cx.theme().warning.opacity(0.14))
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().warning)
                                    .child("T"),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .whitespace_nowrap()
                                            .overflow_hidden()
                                            .text_sm()
                                            .font_semibold()
                                            .child(template.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .whitespace_nowrap()
                                            .overflow_hidden()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(compact_label(&template.payload, 42)),
                                    ),
                            ),
                    )
                    .into_any_element(),
            );
        }
        let no_matches = rows.is_empty()
            && (!self.websocket_workspace.document.messages.is_empty()
                || !self.websocket_workspace.document.templates.is_empty());

        let detail = match selected {
            Some(WebSocketLibrarySelection::Message(id)) => self
                .websocket_workspace
                .document
                .messages
                .iter()
                .find(|message| message.id == id)
                .map(|message| self.render_websocket_saved_message_detail(message, cx))
                .unwrap_or_else(|| self.render_websocket_library_empty(cx)),
            Some(WebSocketLibrarySelection::Template(id)) => self
                .websocket_workspace
                .document
                .templates
                .iter()
                .find(|template| template.id == id)
                .map(|template| self.render_websocket_template_detail(Some(template), cx))
                .unwrap_or_else(|| self.render_websocket_library_empty(cx)),
            Some(WebSocketLibrarySelection::NewTemplate) => {
                self.render_websocket_template_detail(None, cx)
            }
            None => self.render_websocket_library_empty(cx),
        };

        h_flex()
            .id("websocket-messages-workspace")
            .debug_selector(|| "websocket-messages-workspace".to_owned())
            .size_full()
            .min_h_0()
            .child(
                v_flex()
                    .id("websocket-message-library")
                    .debug_selector(|| "websocket-message-library".to_owned())
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
                            .child(div().text_base().font_semibold().child("Messages"))
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        Button::new("quick-send-websocket-template")
                                            .label("Quick send")
                                            .xsmall()
                                            .ghost()
                                            .tooltip("Quick send template")
                                            .disabled(
                                                self.websocket_workspace
                                                    .document
                                                    .templates
                                                    .is_empty(),
                                            )
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.open_websocket_quick_send(window, cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("new-websocket-template")
                                            .icon(IconName::Plus)
                                            .xsmall()
                                            .ghost()
                                            .rounded_full()
                                            .tooltip("New template")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.new_websocket_template(window, cx);
                                            })),
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
                                    Input::new(&self.websocket_workspace.library_search)
                                        .prefix(IconName::Search)
                                        .cleanable(true),
                                ),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scrollbar()
                            .gap_1()
                            .p_2()
                            .children(rows)
                            .when(no_matches, |this| {
                                this.child(
                                    v_flex()
                                        .items_center()
                                        .px_5()
                                        .py_10()
                                        .text_center()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("No matches"),
                                )
                            })
                            .when(
                                self.websocket_workspace.document.messages.is_empty()
                                    && self.websocket_workspace.document.templates.is_empty(),
                                |this| {
                                    this.child(
                                        v_flex()
                                            .items_center()
                                            .gap_1()
                                            .px_5()
                                            .py_10()
                                            .text_center()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(div().text_sm().child("No saved messages"))
                                            .child(
                                                div().text_xs().child(
                                                    "Save from Console or create a template.",
                                                ),
                                            ),
                                    )
                                },
                            ),
                    ),
            )
            .child(
                div()
                    .id("websocket-message-detail")
                    .debug_selector(|| "websocket-message-detail".to_owned())
                    .h_full()
                    .flex_1()
                    .min_w_0()
                    .child(detail),
            )
            .into_any_element()
    }

    fn render_websocket_library_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .p_8()
            .text_center()
            .child(div().text_base().font_semibold().child("Select a message"))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Saved messages and templates open here."),
            )
            .into_any_element()
    }

    fn render_websocket_saved_message_detail(
        &self,
        message: &WebSocketSavedMessage,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let send_payload = message.payload.clone();
        let send_language = message.language;
        let load_id = message.id.clone();
        v_flex()
            .size_full()
            .min_h_0()
            .child(
                h_flex()
                    .h(px(56.))
                    .flex_shrink_0()
                    .gap_2()
                    .px_4()
                    .border_b_1()
                    .border_color(cx.api_outline_variant())
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().font_semibold().child(message.name.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(message.language.label()),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .gap_2()
                            .child(
                                Button::new("load-saved-websocket-message")
                                    .label("Edit in Console")
                                    .small()
                                    .outline()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.load_saved_websocket_message(&load_id, window, cx);
                                    })),
                            )
                            .child(
                                Button::new("delete-saved-websocket-message")
                                    .label("Delete")
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.delete_selected_websocket_library_item(cx);
                                    })),
                            )
                            .child(
                                Button::new("send-saved-websocket-message")
                                    .label("Send")
                                    .small()
                                    .primary()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if send_language == RawBodyLanguage::JsonLines {
                                            this.send_websocket_json_lines(&send_payload, cx);
                                        } else {
                                            this.send_websocket_payload(
                                                send_payload.clone(),
                                                false,
                                                cx,
                                            );
                                        }
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(self.websocket_workspace.library_preview.clone()),
            )
            .into_any_element()
    }

    fn render_websocket_template_detail(
        &self,
        template: Option<&WebSocketMessageTemplate>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let template_id = template.map(|template| template.id.clone());
        let fill_id = template_id.clone();
        v_flex()
            .size_full()
            .min_h_0()
            .child(
                h_flex()
                    .h(px(56.))
                    .flex_shrink_0()
                    .gap_2()
                    .px_4()
                    .border_b_1()
                    .border_color(cx.api_outline_variant())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_semibold()
                            .child(if template.is_some() {
                                "Template"
                            } else {
                                "New template"
                            }),
                    )
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .gap_2()
                            .when_some(fill_id, |this, id| {
                                this.child(
                                    Button::new("fill-websocket-template")
                                        .label("Fill & send")
                                        .small()
                                        .outline()
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.open_websocket_quick_send_for(
                                                id.clone(),
                                                window,
                                                cx,
                                            );
                                        })),
                                )
                            })
                            .when(template.is_some(), |this| {
                                this.child(
                                    Button::new("delete-websocket-template")
                                        .label("Delete")
                                        .small()
                                        .ghost()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.delete_selected_websocket_library_item(cx);
                                        })),
                                )
                            })
                            .child(
                                Button::new("save-websocket-template")
                                    .label(if template.is_some() {
                                        "Save changes"
                                    } else {
                                        "Create"
                                    })
                                    .small()
                                    .primary()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.save_websocket_template(cx);
                                    })),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .h(px(56.))
                    .flex_shrink_0()
                    .px_4()
                    .border_b_1()
                    .border_color(cx.api_outline_variant())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.websocket_workspace.template_name)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(self.websocket_workspace.template_payload.clone()),
            )
            .into_any_element()
    }

    fn render_websocket_console(&self, cx: &mut Context<Self>) -> AnyElement {
        let query = self
            .websocket_workspace
            .timeline_filter
            .read(cx)
            .value()
            .trim()
            .to_owned();
        let direction_filter = self.websocket_workspace.timeline_direction_filter;
        let entries = self
            .websocket_workspace
            .timeline
            .iter()
            .filter(|entry| websocket_timeline_entry_matches(entry, direction_filter, &query))
            .collect::<Vec<_>>();
        let visible_count = entries.len();
        let total_count = self.websocket_workspace.timeline.len();
        let timeline_empty = total_count == 0;
        let no_matches = !timeline_empty && entries.is_empty();
        let following = self.websocket_workspace.timeline_following;
        let scroll_handle = self.websocket_workspace.timeline_scroll.clone();
        let composer_language = self.websocket_workspace.document.composer_language;
        let language_owner = cx.entity().downgrade();
        let language_selector = Button::new("websocket-composer-language")
            .label(composer_language.label())
            .dropdown_caret(true)
            .small()
            .outline()
            .dropdown_menu(move |menu, _, _| {
                RawBodyLanguage::all().iter().copied().fold(
                    menu.min_w(px(180.)).max_h(px(420.)).scrollable(true),
                    |menu, language| {
                        let owner = language_owner.clone();
                        menu.item(
                            PopupMenuItem::new(language.label())
                                .checked(language == composer_language)
                                .on_click(move |_, _, cx| {
                                    if let Some(owner) = owner.upgrade() {
                                        owner.update(cx, |this, cx| {
                                            this.select_websocket_composer_language(language, cx)
                                        });
                                    }
                                }),
                        )
                    },
                )
            });
        v_flex()
            .debug_selector(|| "websocket-console".to_owned())
            .size_full()
            .min_h_0()
            .child(
                h_flex()
                    .gap_2()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.api_outline_variant())
                    .child(
                        div().flex_1().min_w(px(160.)).max_w(px(320.)).child(
                            Input::new(&self.websocket_workspace.timeline_filter)
                                .prefix(IconName::Search)
                                .cleanable(true)
                                .small(),
                        ),
                    )
                    .child(
                        Button::new("websocket-filter-all")
                            .label("All")
                            .outline()
                            .compact()
                            .small()
                            .selected(direction_filter == WebSocketTimelineFilter::All)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.websocket_workspace.timeline_direction_filter =
                                    WebSocketTimelineFilter::All;
                                if this.websocket_workspace.timeline_following {
                                    this.websocket_workspace.timeline_scroll.scroll_to_bottom();
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("websocket-filter-sent")
                            .label("Sent")
                            .outline()
                            .compact()
                            .small()
                            .selected(direction_filter == WebSocketTimelineFilter::Sent)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.websocket_workspace.timeline_direction_filter =
                                    WebSocketTimelineFilter::Sent;
                                if this.websocket_workspace.timeline_following {
                                    this.websocket_workspace.timeline_scroll.scroll_to_bottom();
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("websocket-filter-received")
                            .label("Received")
                            .outline()
                            .compact()
                            .small()
                            .selected(direction_filter == WebSocketTimelineFilter::Received)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.websocket_workspace.timeline_direction_filter =
                                    WebSocketTimelineFilter::Received;
                                if this.websocket_workspace.timeline_following {
                                    this.websocket_workspace.timeline_scroll.scroll_to_bottom();
                                }
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(if visible_count == total_count {
                                format!("{total_count} frames")
                            } else {
                                format!("{visible_count} of {total_count}")
                            }),
                    )
                    .when(!following, |this| {
                        this.child(
                            Button::new("resume-websocket-follow")
                                .label("Resume")
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.websocket_workspace.timeline_following = true;
                                    this.websocket_workspace.timeline_scroll.scroll_to_bottom();
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        Button::new("clear-websocket-timeline")
                            .label("Clear")
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.clear_websocket_timeline();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                h_flex()
                    .h(px(30.))
                    .flex_shrink_0()
                    .px_3()
                    .border_b_1()
                    .border_color(cx.api_outline_variant())
                    .bg(cx.api_surface_low())
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(div().w(px(104.)).child("Type"))
                    .child(div().flex_1().min_w_0().child("Data"))
                    .child(div().w(px(80.)).text_right().child("Length"))
                    .child(div().w(px(112.)).text_right().child("Time")),
            )
            .child(
                div()
                    .debug_selector(|| "websocket-console-split-viewport".to_owned())
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(
                    v_resizable("websocket-console-composer-split")
                        .child(
                            resizable_panel().size_range(px(120.)..px(1_400.)).child(
                                v_flex()
                                    .id("websocket-timeline-scroll")
                                    .size_full()
                                    .min_h_0()
                                    .track_scroll(&scroll_handle)
                                    .overflow_y_scrollbar()
                                    .on_scroll_wheel(cx.listener(
                                        |this, event: &ScrollWheelEvent, window, cx| {
                                            let delta =
                                                event.delta.pixel_delta(window.line_height()).y;
                                            let max_offset = this
                                                .websocket_workspace
                                                .timeline_scroll
                                                .max_offset()
                                                .height;
                                            if max_offset <= px(2.) {
                                                this.websocket_workspace.timeline_following = true;
                                            } else if delta > px(0.) {
                                                this.websocket_workspace.timeline_following = false;
                                            } else if delta < px(0.) {
                                                let handle =
                                                    &this.websocket_workspace.timeline_scroll;
                                                let distance =
                                                    handle.max_offset().height + handle.offset().y;
                                                if distance <= -delta + px(2.) {
                                                    this.websocket_workspace.timeline_following =
                                                        true;
                                                }
                                            }
                                            cx.notify();
                                        },
                                    ))
                                    .when(timeline_empty, |this| {
                                        this.child(
                                            v_flex()
                                                .flex_1()
                                                .items_center()
                                                .justify_center()
                                                .gap_1()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_semibold()
                                                        .child("No messages yet"),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .child("Connect and send a frame."),
                                                ),
                                        )
                                    })
                                    .when(no_matches, |this| {
                                        this.child(
                                            v_flex()
                                                .flex_1()
                                                .items_center()
                                                .justify_center()
                                                .text_sm()
                                                .text_color(cx.theme().muted_foreground)
                                                .child("No matching frames"),
                                        )
                                    })
                                    .children(entries.into_iter().map(|entry| {
                                        let (marker, color) = match entry.direction {
                                            WebSocketTimelineDirection::Sent => {
                                                ("↑", cx.theme().primary)
                                            }
                                            WebSocketTimelineDirection::Received => {
                                                ("↓", cx.theme().success)
                                            }
                                            WebSocketTimelineDirection::System => {
                                                ("•", cx.theme().muted_foreground)
                                            }
                                        };
                                        let selected =
                                            self.websocket_workspace.selected_timeline_entry
                                                == Some(entry.id);
                                        let id = entry.id;
                                        let inspect_payload = entry.payload.clone();
                                        let context_payload = entry.payload.clone();
                                        let context_owner = cx.entity().downgrade();
                                        v_flex()
                                            .id(SharedString::from(format!(
                                                "websocket-frame-container-{}",
                                                entry.id
                                            )))
                                            .border_b_1()
                                            .border_color(cx.api_outline_variant())
                                            .when(selected, |this| this.bg(cx.api_surface_low()))
                                            .child(
                                                h_flex()
                                                    .id(SharedString::from(format!(
                                                        "websocket-frame-row-{}",
                                                        entry.id
                                                    )))
                                                    .min_w_0()
                                                    .px_3()
                                                    .py_2()
                                                    .text_xs()
                                                    .child(
                                                        h_flex()
                                                            .w(px(104.))
                                                            .gap_1()
                                                            .font_semibold()
                                                            .text_color(color)
                                                            .child(marker)
                                                            .child(websocket_timeline_kind_label(
                                                                entry.kind,
                                                            )),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w_0()
                                                            .truncate()
                                                            .font_family(
                                                                cx.theme().mono_font_family.clone(),
                                                            )
                                                            .child(entry.payload.clone()),
                                                    )
                                                    .child(
                                                        div()
                                                            .w(px(80.))
                                                            .text_right()
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child(format!(
                                                                "{} B",
                                                                entry.payload.len()
                                                            )),
                                                    )
                                                    .child(
                                                        div()
                                                            .w(px(112.))
                                                            .text_right()
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child(
                                                                entry
                                                                    .at
                                                                    .format("%H:%M:%S%.3f")
                                                                    .to_string(),
                                                            ),
                                                    )
                                                    .cursor_pointer()
                                                    .on_click(cx.listener(
                                                        move |this, _, window, cx| {
                                                            if this
                                                                .websocket_workspace
                                                                .selected_timeline_entry
                                                                == Some(id)
                                                            {
                                                                this.websocket_workspace
                                                                    .selected_timeline_entry = None;
                                                                cx.notify();
                                                                return;
                                                            }
                                                            let language =
                                                                if serde_json::from_str::<
                                                                    serde_json::Value,
                                                                >(
                                                                    &inspect_payload
                                                                )
                                                                .is_ok()
                                                                {
                                                                    CodeLanguage::Json
                                                                } else {
                                                                    CodeLanguage::Plain
                                                                };
                                                            this.websocket_workspace
                                                                .timeline_preview
                                                                .update(cx, |editor, cx| {
                                                                    editor
                                                                        .set_language(language, cx);
                                                                    editor.set_value(
                                                                        inspect_payload.clone(),
                                                                        window,
                                                                        cx,
                                                                    );
                                                                });
                                                            this.websocket_workspace
                                                                .selected_timeline_entry = Some(id);
                                                            cx.notify();
                                                        },
                                                    )),
                                            )
                                            .when(selected, |this| {
                                                this.child(
                                                    div()
                                                        .h(px(180.))
                                                        .min_h_0()
                                                        .overflow_hidden()
                                                        .border_t_1()
                                                        .border_color(cx.api_outline_variant())
                                                        .child(
                                                            self.websocket_workspace
                                                                .timeline_preview
                                                                .clone(),
                                                        ),
                                                )
                                            })
                                            .context_menu(move |menu, _, _| {
                                                websocket_frame_context_menu(
                                                    menu,
                                                    context_owner.clone(),
                                                    context_payload.clone(),
                                                )
                                            })
                                    })),
                            ),
                        )
                        .child(
                            resizable_panel()
                                .size(px(240.))
                                .size_range(px(140.)..px(720.))
                                .child(
                                    v_flex()
                                        .size_full()
                                        .min_h_0()
                                        .border_t_1()
                                        .border_color(cx.api_outline_variant())
                                        .bg(cx.api_surface_lowest())
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_h_0()
                                                .overflow_hidden()
                                                .child(self.websocket_workspace.composer.clone()),
                                        )
                                        .child(
                                            h_flex()
                                                .debug_selector(|| {
                                                    "websocket-composer-footer".to_owned()
                                                })
                                                .h(px(52.))
                                                .flex_shrink_0()
                                                .gap_2()
                                                .px_3()
                                                .border_t_1()
                                                .border_color(cx.api_outline_variant())
                                                .bg(cx.api_surface_low())
                                                .child(
                                                    Checkbox::new("websocket-reset-input")
                                                        .label("Clear after send")
                                                        .checked(
                                                            self.websocket_workspace
                                                                .document
                                                                .reset_input_after_send,
                                                        )
                                                        .on_click(cx.listener(
                                                            |this, checked: &bool, _, cx| {
                                                                this.websocket_workspace
                                                                    .document
                                                                    .reset_input_after_send =
                                                                    *checked;
                                                                this.persist_websocket_document(cx);
                                                            },
                                                        )),
                                                )
                                                .child(div().flex_1())
                                                .child(language_selector)
                                                .child(
                                                    Button::new("format-websocket-composer")
                                                        .label("Format")
                                                        .small()
                                                        .outline()
                                                        .on_click(cx.listener(
                                                            |this, _, window, cx| {
                                                                this.format_websocket_composer(
                                                                    window, cx,
                                                                )
                                                            },
                                                        )),
                                                )
                                                .child(div().w(px(220.)).child(Input::new(
                                                    &self.websocket_workspace.message_name,
                                                )))
                                                .child(
                                                    Button::new("save-websocket-message")
                                                        .label("Save")
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.save_websocket_message(cx)
                                                        })),
                                                )
                                                .child(
                                                    Button::new("websocket-send")
                                                        .label(
                                                            if composer_language
                                                                == RawBodyLanguage::JsonLines
                                                            {
                                                                "Send all"
                                                            } else {
                                                                "Send"
                                                            },
                                                        )
                                                        .primary()
                                                        .on_click(cx.listener(
                                                            |this, _, window, cx| {
                                                                this.send_composer(window, cx)
                                                            },
                                                        )),
                                                ),
                                        ),
                                ),
                        ),
                ),
            )
            .into_any_element()
    }

    fn render_websocket_replays(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .size_full()
            .min_h_0()
            .overflow_y_scrollbar()
            .p_3()
            .gap_3()
            .child(div().font_semibold().child("Save current session"))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.websocket_workspace.replay_name)),
                    )
                    .child(
                        Button::new("save-websocket-replay")
                            .label("Save replay")
                            .primary()
                            .on_click(cx.listener(|this, _, _, cx| this.save_websocket_replay(cx))),
                    ),
            )
            .children(
                self.websocket_workspace
                    .document
                    .replays
                    .iter()
                    .map(|replay| {
                        let id = replay.id.clone();
                        h_flex()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .border_1()
                            .border_color(cx.api_outline_variant())
                            .child(div().flex_1().child(replay.name.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{} messages", replay.frames.len())),
                            )
                            .child(
                                Button::new(SharedString::from(format!(
                                    "play-replay-{}",
                                    replay.id
                                )))
                                .label("Replay")
                                .small()
                                .on_click(cx.listener(
                                    move |this, _, _, cx| this.play_websocket_replay(&id, cx),
                                )),
                            )
                    }),
            )
            .into_any_element()
    }

    fn render_websocket_automation(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .size_full()
            .min_h_0()
            .items_start()
            .p_4()
            .gap_4()
            .child(
                v_flex()
                    .h_full()
                    .flex_1()
                    .min_w_0()
                    .gap_3()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().font_semibold().child("Automation"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Runs when the connection opens or a message arrives."),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .rounded_lg()
                            .border_1()
                            .border_color(cx.api_outline_variant())
                            .overflow_hidden()
                            .child(self.websocket_workspace.automation.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Checkbox::new("websocket-automation-enabled")
                                    .label("Enable automation")
                                    .checked(self.websocket_workspace.document.automation_enabled)
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.websocket_workspace.document.automation_enabled =
                                            *checked;
                                        this.persist_websocket_document(cx);
                                    })),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new("save-websocket-automation")
                                    .label("Apply")
                                    .primary()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.persist_websocket_document(cx);
                                    })),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .h_full()
                    .w(px(380.))
                    .flex_shrink_0()
                    .min_h_0()
                    .gap_3()
                    .child(div().font_semibold().child("Connection headers"))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .rounded_lg()
                            .border_1()
                            .border_color(cx.api_outline_variant())
                            .overflow_hidden()
                            .child(self.websocket_workspace.headers.clone()),
                    ),
            )
            .into_any_element()
    }
}

fn websocket_frame_context_menu(
    menu: PopupMenu,
    owner: WeakEntity<ApiTester>,
    payload: String,
) -> PopupMenu {
    let copy_all_owner = owner.clone();
    menu.item(
        PopupMenuItem::new("Copy message").on_click(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(payload.clone()));
        }),
    )
    .item(
        PopupMenuItem::new("Copy all messages").on_click(move |_, _, cx| {
            if let Some(owner) = copy_all_owner.upgrade() {
                owner.update(cx, |this, cx| {
                    let all_messages = this
                        .websocket_workspace
                        .timeline
                        .iter()
                        .map(|entry| entry.payload.as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    cx.write_to_clipboard(ClipboardItem::new_string(all_messages));
                });
            }
        }),
    )
    .separator()
    .item(PopupMenuItem::new("Clear").on_click(move |_, _, cx| {
        if let Some(owner) = owner.upgrade() {
            owner.update(cx, |this, cx| {
                this.clear_websocket_timeline();
                cx.notify();
            });
        }
    }))
}

fn websocket_timeline_entry_matches(
    entry: &WebSocketTimelineEntry,
    direction_filter: WebSocketTimelineFilter,
    query: &str,
) -> bool {
    let direction_matches = match direction_filter {
        WebSocketTimelineFilter::All => true,
        WebSocketTimelineFilter::Sent => entry.direction == WebSocketTimelineDirection::Sent,
        WebSocketTimelineFilter::Received => {
            entry.direction == WebSocketTimelineDirection::Received
        }
    };
    if !direction_matches || query.is_empty() {
        return direction_matches;
    }

    let direction = match entry.direction {
        WebSocketTimelineDirection::Sent => "sent",
        WebSocketTimelineDirection::Received => "received",
        WebSocketTimelineDirection::System => "system",
    };
    contains_ascii_case_insensitive(direction, query)
        || contains_ascii_case_insensitive(entry.kind, query)
        || contains_ascii_case_insensitive(&entry.payload, query)
}

fn websocket_library_item_matches(name: &str, payload: &str, query: &str) -> bool {
    query.is_empty()
        || contains_ascii_case_insensitive(name, query)
        || contains_ascii_case_insensitive(payload, query)
}

fn websocket_timeline_kind_label(kind: &'static str) -> &'static str {
    match kind {
        "open" => "Open",
        "text" => "Text",
        "binary" => "Binary",
        "ping" => "Ping",
        "pong" => "Pong",
        "close" => "Close",
        "error" => "Error",
        "script" => "Script",
        "script error" => "Script error",
        other => other,
    }
}

fn contains_ascii_case_insensitive(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

fn websocket_scroll_is_at_bottom(handle: &ScrollHandle) -> bool {
    handle.max_offset().height + handle.offset().y <= px(2.)
}

fn parse_websocket_headers(source: &str) -> Result<Vec<HeaderEntry>, String> {
    if source.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(source)
        .map_err(|error| format!("Connection headers are not valid JSON: {error}"))
}

fn new_websocket_id(prefix: &str) -> String {
    format!("{prefix}-{}", Utc::now().timestamp_micros())
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, VisualTestContext, px, size};

    use super::*;

    fn mount_app(
        cx: &mut TestAppContext,
    ) -> (Entity<ApiTester>, &mut VisualTestContext, tempfile::TempDir) {
        let directory = tempfile::tempdir().expect("create temporary database directory");
        let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
        store.initialize().expect("initialize test database");

        let mut app = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            let base_key_bindings = shortcuts::capture_base_key_bindings(cx);
            crate::theme::configure(cx);
            let view = cx
                .new(|cx| ApiTester::new_with_database_store(base_key_bindings, store, window, cx));
            crate::register_app_action_handlers(&view, cx);
            app = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        (app.expect("capture app entity"), visual, directory)
    }

    #[test]
    fn parses_explicit_connection_header_rows() {
        let rows = parse_websocket_headers(
            r#"[{"enabled":true,"shared":true,"name":"Authorization","value":"Bearer {{token}}"}]"#,
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Authorization");
    }

    #[test]
    fn timeline_filter_matches_direction_kind_and_payload() {
        let entry = WebSocketTimelineEntry {
            id: 1,
            direction: WebSocketTimelineDirection::Received,
            at: Utc::now(),
            kind: "text",
            payload: r#"{"event":"ready"}"#.to_owned(),
        };

        assert!(websocket_timeline_entry_matches(
            &entry,
            WebSocketTimelineFilter::All,
            "READY"
        ));
        assert!(websocket_timeline_entry_matches(
            &entry,
            WebSocketTimelineFilter::Received,
            "text"
        ));
        assert!(!websocket_timeline_entry_matches(
            &entry,
            WebSocketTimelineFilter::Sent,
            ""
        ));
    }

    #[test]
    fn message_library_filter_matches_names_and_payloads_case_insensitively() {
        assert!(websocket_library_item_matches(
            "Authenticate guest",
            r#"{"type":"auth"}"#,
            "GUEST"
        ));
        assert!(websocket_library_item_matches(
            "Authenticate guest",
            r#"{"type":"auth"}"#,
            "AUTH"
        ));
        assert!(!websocket_library_item_matches(
            "Authenticate guest",
            r#"{"type":"auth"}"#,
            "subscribe"
        ));
    }

    #[test]
    fn timeline_labels_every_supported_frame_and_event_type() {
        assert_eq!(websocket_timeline_kind_label("open"), "Open");
        assert_eq!(websocket_timeline_kind_label("text"), "Text");
        assert_eq!(websocket_timeline_kind_label("binary"), "Binary");
        assert_eq!(websocket_timeline_kind_label("ping"), "Ping");
        assert_eq!(websocket_timeline_kind_label("pong"), "Pong");
        assert_eq!(websocket_timeline_kind_label("close"), "Close");
        assert_eq!(websocket_timeline_kind_label("error"), "Error");
        assert_eq!(websocket_timeline_kind_label("script"), "Script");
        assert_eq!(
            websocket_timeline_kind_label("script error"),
            "Script error"
        );
    }

    #[gpui::test]
    fn composer_footer_stays_inside_the_split_at_short_window_heights(cx: &mut TestAppContext) {
        let (app, cx, _directory) = mount_app(cx);
        cx.simulate_resize(size(px(1_200.), px(620.)));
        cx.update(|window, cx| {
            app.update(cx, |app, cx| app.open_blank_websocket_tab(window, cx));
        });
        cx.run_until_parked();

        let console = cx
            .debug_bounds("websocket-console")
            .expect("WebSocket console should be laid out");
        let split_viewport = cx
            .debug_bounds("websocket-console-split-viewport")
            .expect("WebSocket split viewport should be laid out");
        let footer = cx
            .debug_bounds("websocket-composer-footer")
            .expect("WebSocket composer footer should be laid out");

        assert!(split_viewport.is_contained_within(&console));
        assert!(footer.is_contained_within(&split_viewport));
    }

    #[gpui::test]
    fn message_library_and_detail_stay_inside_the_workspace(cx: &mut TestAppContext) {
        let (app, cx, _directory) = mount_app(cx);
        cx.simulate_resize(size(px(900.), px(560.)));
        cx.update(|window, cx| {
            app.update(cx, |app, cx| app.open_blank_websocket_tab(window, cx));
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.websocket_workspace.section = WebSocketSection::Messages;
                app.websocket_workspace
                    .document
                    .messages
                    .push(WebSocketSavedMessage {
                        id: "message-one".to_owned(),
                        name: "Authenticate".to_owned(),
                        payload: r#"{"type":"auth"}"#.to_owned(),
                        language: RawBodyLanguage::Json,
                    });
                app.select_websocket_library_message("message-one".to_owned(), window, cx);
            });
        });
        cx.run_until_parked();

        cx.update(|_, cx| {
            assert_eq!(
                app.read(cx).websocket_workspace.section,
                WebSocketSection::Messages
            );
        });

        let workspace = cx
            .debug_bounds("websocket-messages-workspace")
            .expect("message workspace should be laid out");
        let library = cx
            .debug_bounds("websocket-message-library")
            .expect("message library should be laid out");
        let detail = cx
            .debug_bounds("websocket-message-detail")
            .expect("message detail should be laid out");

        assert!(library.is_contained_within(&workspace));
        assert!(detail.is_contained_within(&workspace));
        assert!(library.right() <= detail.left());
    }

    #[gpui::test]
    fn quick_send_shortcut_opens_picker_and_enter_opens_the_fill_form(cx: &mut TestAppContext) {
        let (app, cx, _directory) = mount_app(cx);
        cx.simulate_resize(size(px(900.), px(560.)));
        cx.update(|window, _| window.activate_window());
        cx.update(|window, cx| {
            app.update(cx, |app, cx| app.open_blank_websocket_tab(window, cx));
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.websocket_workspace
                    .document
                    .templates
                    .push(WebSocketMessageTemplate {
                        id: "template-one".to_owned(),
                        name: "Introduce".to_owned(),
                        payload: r#"{"first":"%{first}%","second":"%{second}%"}"#.to_owned(),
                    });
                cx.notify();
            });
        });
        cx.run_until_parked();

        cx.simulate_keystrokes("secondary-shift-enter");
        cx.run_until_parked();
        assert_eq!(
            cx.update(|_, cx| app.read(cx).websocket_workspace.quick_send_stage),
            WebSocketQuickSendStage::Picker
        );
        assert!(cx.debug_bounds("websocket-quick-send").is_some());

        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let field_names = cx.update(|_, cx| {
            app.read(cx)
                .websocket_workspace
                .template_values
                .iter()
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>()
        });
        assert_eq!(field_names, ["first", "second"]);

        let workspace = cx
            .debug_bounds("websocket-workspace")
            .expect("WebSocket workspace should be laid out");
        let quick_send = cx
            .debug_bounds("websocket-quick-send")
            .expect("Quick send form should be laid out");
        let footer = cx
            .debug_bounds("websocket-quick-send-footer")
            .expect("Quick send footer should be laid out");
        assert!(quick_send.is_contained_within(&workspace));
        assert!(footer.is_contained_within(&quick_send));

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!cx.update(|_, cx| app.read(cx).websocket_workspace.quick_send_open));
    }

    #[gpui::test]
    fn quick_send_arrow_keys_move_selection_and_enter_uses_it(cx: &mut TestAppContext) {
        let (app, cx, _directory) = mount_app(cx);
        cx.update(|window, _| window.activate_window());
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_blank_websocket_tab(window, cx);
                for (id, name) in [
                    ("template-one", "One"),
                    ("template-two", "Two"),
                    ("template-three", "Three"),
                ] {
                    app.websocket_workspace
                        .document
                        .templates
                        .push(WebSocketMessageTemplate {
                            id: id.to_owned(),
                            name: name.to_owned(),
                            payload: format!(r#"{{"message":"%{{{name}}}%"}}"#),
                        });
                }
                app.open_websocket_quick_send(window, cx);
            });
        });
        cx.run_until_parked();

        assert_eq!(
            cx.update(|_, cx| {
                app.read(cx)
                    .websocket_workspace
                    .quick_send_selected_template_id
                    .clone()
            }),
            Some("template-one".to_owned())
        );

        cx.update(|window, cx| {
            let query = app.read(cx).websocket_workspace.quick_send_query.clone();
            query.update(cx, |input, cx| input.set_value("Three", window, cx));
        });
        cx.run_until_parked();
        assert_eq!(
            cx.update(|_, cx| {
                app.read(cx)
                    .websocket_workspace
                    .quick_send_selected_template_id
                    .clone()
            }),
            Some("template-three".to_owned())
        );
        cx.update(|window, cx| {
            let query = app.read(cx).websocket_workspace.quick_send_query.clone();
            query.update(cx, |input, cx| input.set_value("", window, cx));
        });
        cx.run_until_parked();

        cx.simulate_keystrokes("down down up enter");
        cx.run_until_parked();

        cx.update(|_, cx| {
            let state = &app.read(cx).websocket_workspace;
            assert_eq!(state.quick_send_stage, WebSocketQuickSendStage::Fill);
            assert_eq!(state.active_template_id.as_deref(), Some("template-two"));
        });
    }

    #[gpui::test]
    fn request_send_shortcut_submits_the_quick_send_form(cx: &mut TestAppContext) {
        let (app, cx, _directory) = mount_app(cx);
        cx.update(|window, _| window.activate_window());
        cx.update(|window, cx| {
            app.update(cx, |app, cx| app.open_blank_websocket_tab(window, cx));
        });
        cx.run_until_parked();
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.websocket_workspace
                    .document
                    .templates
                    .push(WebSocketMessageTemplate {
                        id: "template-send".to_owned(),
                        name: "Greet".to_owned(),
                        payload: r#"{"message":"%{message}%"}"#.to_owned(),
                    });
                app.websocket_workspace.command_sender = Some(sender);
                app.websocket_workspace.status = WebSocketConnectionStatus::Connected;
                app.open_websocket_quick_send_for("template-send".to_owned(), window, cx);
                app.websocket_workspace.template_values[0]
                    .1
                    .update(cx, |input, cx| input.set_value("hello", window, cx));
            });
        });
        cx.run_until_parked();

        cx.simulate_keystrokes("secondary-enter");
        cx.run_until_parked();

        match receiver.try_recv().expect("quick send should emit a frame") {
            WebSocketCommand::SendText(payload) => {
                assert_eq!(payload, r#"{"message":"hello"}"#)
            }
            other => panic!("expected a text frame, got {other:?}"),
        }
        assert!(!cx.update(|_, cx| app.read(cx).websocket_workspace.quick_send_open));
    }
}
