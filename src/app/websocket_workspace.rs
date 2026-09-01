use std::{
    collections::{BTreeMap, HashMap},
    time::Instant,
};

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
enum WebSocketTimelineDirection {
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

struct WebSocketTimelineEntry {
    id: u64,
    direction: WebSocketTimelineDirection,
    at: chrono::DateTime<Utc>,
    kind: &'static str,
    payload: String,
}

pub(in crate::app) struct WebSocketWorkspaceState {
    pub(in crate::app) document: WebSocketWorkspace,
    pub(in crate::app) url: Entity<InputState>,
    pub(in crate::app) headers: Entity<CodeEditor>,
    pub(in crate::app) composer: Entity<CodeEditor>,
    message_name: Entity<InputState>,
    template_name: Entity<InputState>,
    template_payload: Entity<CodeEditor>,
    replay_name: Entity<InputState>,
    pub(in crate::app) automation: Entity<CodeEditor>,
    section: WebSocketSection,
    status: WebSocketConnectionStatus,
    notice: Option<String>,
    timeline: Vec<WebSocketTimelineEntry>,
    pub(in crate::app) timeline_filter: Entity<InputState>,
    timeline_direction_filter: WebSocketTimelineFilter,
    pub(in crate::app) timeline_scroll: ScrollHandle,
    pub(in crate::app) timeline_following: bool,
    selected_timeline_entry: Option<u64>,
    pub(in crate::app) timeline_preview: Entity<CodeEditor>,
    next_timeline_entry_id: u64,
    sent_session: Vec<(Instant, String)>,
    active_template_id: Option<String>,
    template_values: HashMap<String, Entity<InputState>>,
    command_sender: Option<tokio::sync::mpsc::UnboundedSender<WebSocketCommand>>,
    abort_handle: Option<AbortHandle>,
    generation: u64,
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
            template_values: HashMap::new(),
            command_sender: None,
            abort_handle: None,
            generation: 0,
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
        self.stop_websocket();
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
            self.websocket_workspace.notice = Some(format!("The {label} buffer is empty."));
            cx.notify();
            return;
        }
        let formatted = match format_raw_body_source(language, &source, &self.settings.formatter) {
            Ok(formatted) => formatted,
            Err(message) => {
                self.websocket_workspace.notice = Some(message);
                cx.notify();
                return;
            }
        };
        if formatted == source {
            self.websocket_workspace.notice = Some(format!("The {label} is already formatted."));
            cx.notify();
            return;
        }
        editor.update(cx, |editor, cx| editor.set_value(formatted, window, cx));
        self.websocket_workspace.notice = Some(format!("Formatted {label} as {language}."));
        cx.notify();
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

    fn connect_websocket(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_websocket();
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
        let (command_sender, command_receiver) = tokio::sync::mpsc::unbounded_channel();
        let (signal_sender, mut signal_receiver) = tokio::sync::mpsc::unbounded_channel();
        let generation = self.websocket_workspace.generation;
        self.websocket_workspace.command_sender = Some(command_sender);
        self.websocket_workspace.status = WebSocketConnectionStatus::Connecting;
        self.websocket_workspace.notice = None;
        self.websocket_workspace.sent_session.clear();
        let task = self.runtime.spawn(async move {
            if let Err(error) =
                run_websocket_connection(&url, &headers, command_receiver, signal_sender.clone())
                    .await
            {
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
        self.push_websocket_timeline(WebSocketTimelineDirection::Sent, "text", payload);
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
                        row: source[..record.range.end]
                            .bytes()
                            .filter(|byte| *byte == b'\n')
                            .count(),
                        label: "▷ Send".into(),
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
        self.websocket_workspace
            .document
            .templates
            .push(WebSocketMessageTemplate {
                id: new_websocket_id("template"),
                name,
                payload,
            });
        self.persist_websocket_document(cx);
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
        self.websocket_workspace.template_values = names
            .into_iter()
            .map(|name| {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder(name.clone()));
                (name, input)
            })
            .collect();
        cx.notify();
    }

    fn send_active_websocket_template(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.websocket_workspace.active_template_id.as_deref() else {
            return;
        };
        let Some(template) = self
            .websocket_workspace
            .document
            .templates
            .iter()
            .find(|item| item.id == id)
        else {
            return;
        };
        let values = self
            .websocket_workspace
            .template_values
            .iter()
            .map(|(name, input)| (name.clone(), input.read(cx).value().to_string()))
            .collect::<BTreeMap<_, _>>();
        match render_message_template(&template.payload, &values) {
            Ok(payload) => self.send_websocket_payload(payload, false, cx),
            Err(error) => {
                self.websocket_workspace.notice = Some(error.to_string());
                cx.notify();
            }
        }
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
            .when_some(self.websocket_workspace.notice.clone(), |this, notice| {
                this.child(
                    div()
                        .mx_4()
                        .mt_2()
                        .min_w_0()
                        .flex_shrink_0()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(cx.theme().danger.opacity(0.08))
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(div().w_full().whitespace_normal().child(notice)),
                )
            })
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
            .into_any_element()
    }

    fn render_websocket_messages(&self, cx: &mut Context<Self>) -> AnyElement {
        let message_rows = self
            .websocket_workspace
            .document
            .messages
            .iter()
            .map(|message| {
                let payload = message.payload.clone();
                let language = message.language;
                h_flex()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.api_outline_variant())
                    .child(
                        div()
                            .w(px(180.))
                            .font_semibold()
                            .child(message.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .child(message.payload.clone()),
                    )
                    .child(
                        Button::new(SharedString::from(format!("send-saved-{}", message.id)))
                            .label("Send")
                            .small()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if language == RawBodyLanguage::JsonLines {
                                    this.send_websocket_json_lines(&payload, cx);
                                } else {
                                    this.send_websocket_payload(payload.clone(), false, cx);
                                }
                            })),
                    )
            })
            .collect::<Vec<_>>();
        let template_rows = self
            .websocket_workspace
            .document
            .templates
            .iter()
            .map(|template| {
                let id = template.id.clone();
                h_flex()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.api_outline_variant())
                    .child(
                        div()
                            .w(px(180.))
                            .font_semibold()
                            .child(template.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .child(template.payload.clone()),
                    )
                    .child(
                        Button::new(SharedString::from(format!("use-template-{}", template.id)))
                            .label("Fill & send")
                            .small()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_websocket_template(id.clone(), window, cx)
                            })),
                    )
            })
            .collect::<Vec<_>>();
        let template_inputs = self
            .websocket_workspace
            .active_template_id
            .as_ref()
            .map(|_| {
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.api_outline_variant())
                    .child(div().font_semibold().child("Template values"))
                    .children(self.websocket_workspace.template_values.iter().map(
                        |(name, input)| {
                            h_flex()
                                .gap_2()
                                .child(div().w(px(140.)).text_sm().child(name.clone()))
                                .child(div().flex_1().child(Input::new(input)))
                        },
                    ))
                    .child(
                        Button::new("send-filled-template")
                            .label("Send rendered message")
                            .primary()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.send_active_websocket_template(cx)
                            })),
                    )
            });
        v_flex()
            .size_full()
            .min_h_0()
            .overflow_y_scrollbar()
            .p_4()
            .gap_4()
            .children(template_inputs)
            .child(
                h_flex()
                    .child(div().flex_1().font_semibold().child("Saved messages"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Send directly from your library"),
                    ),
            )
            .children(message_rows)
            .child(
                h_flex()
                    .pt_2()
                    .child(div().flex_1().font_semibold().child("Templates"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Use %{name}% for prompted values"),
                    ),
            )
            .child(
                div()
                    .h(px(140.))
                    .overflow_hidden()
                    .child(self.websocket_workspace.template_payload.clone()),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.websocket_workspace.template_name)),
                    )
                    .child(
                        Button::new("save-websocket-template")
                            .label("Save template")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.save_websocket_template(cx)),
                            ),
                    ),
            )
            .children(template_rows)
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
                    .child(div().w(px(72.)).child("Type"))
                    .child(div().flex_1().min_w_0().child("Data"))
                    .child(div().w(px(80.)).text_right().child("Length"))
                    .child(div().w(px(112.)).text_right().child("Time")),
            )
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
                                        let delta = event.delta.pixel_delta(window.line_height()).y;
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
                                            let handle = &this.websocket_workspace.timeline_scroll;
                                            let distance =
                                                handle.max_offset().height + handle.offset().y;
                                            if distance <= -delta + px(2.) {
                                                this.websocket_workspace.timeline_following = true;
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
                                                div().text_xs().child("Connect and send a frame."),
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
                                    let (direction, marker, color) = match entry.direction {
                                        WebSocketTimelineDirection::Sent => {
                                            ("Sent", "↑", cx.theme().primary)
                                        }
                                        WebSocketTimelineDirection::Received => {
                                            ("Received", "↓", cx.theme().success)
                                        }
                                        WebSocketTimelineDirection::System => {
                                            ("System", "•", cx.theme().muted_foreground)
                                        }
                                    };
                                    let selected = self.websocket_workspace.selected_timeline_entry
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
                                                        .w(px(72.))
                                                        .gap_1()
                                                        .font_semibold()
                                                        .text_color(color)
                                                        .child(marker)
                                                        .child(direction),
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
                                                        let language = if serde_json::from_str::<
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
                                                                editor.set_language(language, cx);
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
                                                                .reset_input_after_send = *checked;
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
    use super::*;

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
}
