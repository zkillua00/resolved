use std::{
    collections::{BTreeMap, HashMap},
    time::Instant,
};

use super::*;
use base64::Engine as _;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum WebSocketSection {
    #[default]
    Messages,
    Timeline,
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

struct WebSocketTimelineEntry {
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
                    .language(CodeLanguage::Json)
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
            section: WebSocketSection::Messages,
            status: WebSocketConnectionStatus::Disconnected,
            notice: None,
            timeline: Vec::new(),
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
            editor.set_value(document.composer.clone(), window, cx)
        });
        self.websocket_workspace
            .automation
            .update(cx, |editor, cx| {
                editor.set_value(document.automation_source.clone(), window, cx)
            });
        self.websocket_workspace.document = document;
        self.websocket_workspace.notice = None;
        self.websocket_workspace.timeline.clear();
        self.websocket_workspace.sent_session.clear();
        self.websocket_workspace.active_template_id = None;
        self.websocket_workspace.template_values.clear();
        self.websocket_workspace.hydrating = false;
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
        self.websocket_workspace
            .timeline
            .push(WebSocketTimelineEntry {
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
        self.send_websocket_payload(payload, false, cx);
        if self.websocket_workspace.document.reset_input_after_send
            && self.websocket_workspace.notice.is_none()
        {
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
                    .gap_2()
                    .p_3()
                    .border_b_1()
                    .border_color(cx.api_outline_variant())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.websocket_workspace.url)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(if connected {
                                cx.theme().success
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(status),
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
                        .mx_3()
                        .mt_2()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(cx.theme().danger.opacity(0.08))
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(notice),
                )
            })
            .child(
                TabBar::new("websocket-sections")
                    .underline()
                    .children(["Messages", "Send / receive", "Replays", "Automation"])
                    .selected_index(match self.websocket_workspace.section {
                        WebSocketSection::Messages => 0,
                        WebSocketSection::Timeline => 1,
                        WebSocketSection::Replays => 2,
                        WebSocketSection::Automation => 3,
                    })
                    .on_click(cx.listener(|this, index: &usize, _, cx| {
                        this.websocket_workspace.section = match index {
                            1 => WebSocketSection::Timeline,
                            2 => WebSocketSection::Replays,
                            3 => WebSocketSection::Automation,
                            _ => WebSocketSection::Messages,
                        };
                        cx.notify();
                    })),
            )
            .child(div().flex_1().min_h_0().overflow_hidden().child(
                match self.websocket_workspace.section {
                    WebSocketSection::Messages => self.render_websocket_messages(cx),
                    WebSocketSection::Timeline => self.render_websocket_timeline(cx),
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
                                this.send_websocket_payload(payload.clone(), false, cx)
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
            .p_3()
            .gap_3()
            .child(div().font_semibold().child("Composer"))
            .child(self.websocket_workspace.composer.clone())
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("websocket-send")
                            .label("Send")
                            .primary()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.send_composer(window, cx)),
                            ),
                    )
                    .child(
                        Checkbox::new("websocket-reset-input")
                            .label("Reset input after send")
                            .checked(self.websocket_workspace.document.reset_input_after_send)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.websocket_workspace.document.reset_input_after_send = *checked;
                                this.persist_websocket_document(cx);
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .w(px(220.))
                            .child(Input::new(&self.websocket_workspace.message_name)),
                    )
                    .child(
                        Button::new("save-websocket-message")
                            .label("Save message")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.save_websocket_message(cx)),
                            ),
                    ),
            )
            .children(template_inputs)
            .child(div().pt_2().font_semibold().child("Saved messages"))
            .children(message_rows)
            .child(div().pt_2().font_semibold().child("Message templates"))
            .child(self.websocket_workspace.template_payload.clone())
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

    fn render_websocket_timeline(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .size_full()
            .min_h_0()
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.api_outline_variant())
                    .child(div().font_semibold().child(format!(
                        "{} wire events",
                        self.websocket_workspace.timeline.len()
                    )))
                    .child(div().flex_1())
                    .child(
                        Button::new("clear-websocket-timeline")
                            .label("Clear")
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.websocket_workspace.timeline.clear();
                                cx.notify();
                            })),
                    ),
            )
            .child(v_flex().flex_1().min_h_0().overflow_y_scrollbar().children(
                self.websocket_workspace.timeline.iter().rev().map(|entry| {
                    let direction = match entry.direction {
                        WebSocketTimelineDirection::Sent => "SEND",
                        WebSocketTimelineDirection::Received => "RECV",
                        WebSocketTimelineDirection::System => "SYSTEM",
                    };
                    v_flex()
                        .gap_1()
                        .px_3()
                        .py_2()
                        .border_b_1()
                        .border_color(cx.api_outline_variant())
                        .child(
                            h_flex()
                                .gap_2()
                                .text_xs()
                                .child(div().font_semibold().child(direction))
                                .child(
                                    div()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(entry.kind),
                                )
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(entry.at.format("%H:%M:%S%.3f").to_string()),
                                ),
                        )
                        .child(div().text_sm().child(entry.payload.clone()))
                }),
            ))
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
        v_flex()
            .size_full()
            .min_h_0()
            .p_3()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Run a script when the connection opens or a message arrives."),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
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
                                this.websocket_workspace.document.automation_enabled = *checked;
                                this.persist_websocket_document(cx);
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("save-websocket-automation")
                            .label("Apply")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.persist_websocket_document(cx);
                            })),
                    ),
            )
            .child(div().font_semibold().child("Connection headers"))
            .child(self.websocket_workspace.headers.clone())
            .into_any_element()
    }
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
}
