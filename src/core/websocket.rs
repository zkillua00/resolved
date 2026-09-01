//! First-class WebSocket request documents and bounded runtime helpers.
//!
//! The persisted document is deliberately separate from the live socket. It
//! records reusable intent (messages, templates, automation and replays), while
//! the UI owns the ephemeral connection and wire timeline.

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use futures::{SinkExt as _, StreamExt as _};
use rquickjs::{Context as JsContext, Runtime as JsRuntime};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{
        Message,
        client::IntoClientRequest as _,
        http::{HeaderName, HeaderValue},
        protocol::WebSocketConfig,
    },
};

use super::{HeaderEntry, RawBodyLanguage};

pub const MAX_WEBSOCKET_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_WEBSOCKET_TIMELINE_ENTRIES: usize = 2_000;
const AUTOMATION_MEMORY_BYTES: usize = 16 * 1024 * 1024;
const AUTOMATION_STACK_BYTES: usize = 256 * 1024;
const AUTOMATION_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct WebSocketWorkspace {
    pub url: String,
    pub headers: Vec<HeaderEntry>,
    pub composer: String,
    pub composer_language: RawBodyLanguage,
    pub messages: Vec<WebSocketSavedMessage>,
    pub templates: Vec<WebSocketMessageTemplate>,
    pub replays: Vec<WebSocketReplay>,
    pub reset_input_after_send: bool,
    pub automation_enabled: bool,
    pub automation_source: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct WebSocketSavedMessage {
    pub id: String,
    pub name: String,
    pub payload: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct WebSocketMessageTemplate {
    pub id: String,
    pub name: String,
    pub payload: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct WebSocketReplay {
    pub id: String,
    pub name: String,
    pub frames: Vec<WebSocketReplayFrame>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct WebSocketReplayFrame {
    pub delay_ms: u64,
    pub payload: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebSocketCommand {
    SendText(String),
    #[allow(dead_code)]
    SendBinary(Vec<u8>),
    Close,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebSocketSignal {
    Connected,
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Closed(Option<String>),
    Failed(String),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WebSocketAutomationEvent {
    pub event_type: String,
    pub data: Option<String>,
    pub binary_base64: Option<String>,
}

impl WebSocketAutomationEvent {
    pub fn opened() -> Self {
        Self {
            event_type: "open".to_owned(),
            data: None,
            binary_base64: None,
        }
    }

    pub fn text(data: impl Into<String>) -> Self {
        Self {
            event_type: "message".to_owned(),
            data: Some(data.into()),
            binary_base64: None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WebSocketAutomationOutput {
    #[serde(default)]
    pub sends: Vec<String>,
    #[serde(default)]
    pub logs: Vec<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum WebSocketDocumentError {
    #[error("template variable name cannot be empty")]
    EmptyTemplateVariable,
    #[error("unclosed template variable at byte {0}")]
    UnclosedTemplateVariable(usize),
    #[error("template variable '{0}' has no value")]
    MissingTemplateValue(String),
}

#[derive(Debug, Error)]
pub enum WebSocketConnectError {
    #[error("invalid WebSocket URL: {0}")]
    InvalidUrl(String),
    #[error("WebSocket URLs must use ws:// or wss://")]
    UnsupportedScheme,
    #[error("invalid header name '{name}': {reason}")]
    InvalidHeaderName { name: String, reason: String },
    #[error("invalid value for header '{name}': {reason}")]
    InvalidHeaderValue { name: String, reason: String },
}

pub fn template_variable_names(source: &str) -> Result<Vec<String>, WebSocketDocumentError> {
    let mut names = Vec::new();
    let mut cursor = 0;
    while let Some(relative_start) = source[cursor..].find("%{") {
        let start = cursor + relative_start;
        let value_start = start + 2;
        let Some(relative_end) = source[value_start..].find("}%") else {
            return Err(WebSocketDocumentError::UnclosedTemplateVariable(start));
        };
        let end = value_start + relative_end;
        let name = source[value_start..end].trim();
        if name.is_empty() {
            return Err(WebSocketDocumentError::EmptyTemplateVariable);
        }
        if !names.iter().any(|candidate| candidate == name) {
            names.push(name.to_owned());
        }
        cursor = end + 2;
    }
    Ok(names)
}

pub fn render_message_template(
    source: &str,
    values: &BTreeMap<String, String>,
) -> Result<String, WebSocketDocumentError> {
    let names = template_variable_names(source)?;
    for name in names {
        if !values.contains_key(&name) {
            return Err(WebSocketDocumentError::MissingTemplateValue(name));
        }
    }

    let mut rendered = String::with_capacity(source.len());
    let mut cursor = 0;
    while let Some(relative_start) = source[cursor..].find("%{") {
        let start = cursor + relative_start;
        let value_start = start + 2;
        let relative_end = source[value_start..]
            .find("}%")
            .expect("validated template contains a closing marker");
        let end = value_start + relative_end;
        let name = source[value_start..end].trim();
        rendered.push_str(&source[cursor..start]);
        rendered.push_str(values.get(name).expect("validated template value exists"));
        cursor = end + 2;
    }
    rendered.push_str(&source[cursor..]);
    Ok(rendered)
}

pub fn replay_frames(sent: &[(Instant, String)]) -> Vec<WebSocketReplayFrame> {
    let mut previous = None;
    sent.iter()
        .map(|(at, payload)| {
            let delay_ms = previous
                .map(|last: Instant| at.saturating_duration_since(last).as_millis() as u64)
                .unwrap_or(0);
            previous = Some(*at);
            WebSocketReplayFrame {
                delay_ms,
                payload: payload.clone(),
            }
        })
        .collect()
}

pub fn execute_websocket_automation(
    source: &str,
    event: &WebSocketAutomationEvent,
    environment: &BTreeMap<String, String>,
) -> Result<WebSocketAutomationOutput, String> {
    if source.trim().is_empty() {
        return Ok(WebSocketAutomationOutput::default());
    }
    let runtime = JsRuntime::new().map_err(|error| error.to_string())?;
    runtime.set_memory_limit(AUTOMATION_MEMORY_BYTES);
    runtime.set_max_stack_size(AUTOMATION_STACK_BYTES);
    let deadline = Instant::now() + AUTOMATION_TIMEOUT;
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
    let context = JsContext::full(&runtime).map_err(|error| error.to_string())?;
    context.with(|ctx| {
        let event =
            rquickjs_serde::to_value(ctx.clone(), event).map_err(|error| error.to_string())?;
        let environment = rquickjs_serde::to_value(ctx.clone(), environment)
            .map_err(|error| error.to_string())?;
        ctx.globals()
            .set("__WS_EVENT", event)
            .map_err(|error| error.to_string())?;
        ctx.globals()
            .set("__WS_ENV", environment)
            .map_err(|error| error.to_string())?;
        ctx.eval::<(), _>(AUTOMATION_PRELUDE)
            .map_err(|error| error.to_string())?;
        ctx.eval::<(), _>(source)
            .map_err(|error| error.to_string())?;
        let output = ctx
            .eval::<rquickjs::Value<'_>, _>("__WS_FINISH()")
            .map_err(|error| error.to_string())?;
        rquickjs_serde::from_value_strict(output).map_err(|error| error.to_string())
    })
}

const AUTOMATION_PRELUDE: &str = r#"
(() => {
  "use strict";
  const sends = [];
  const logs = [];
  const environment = Object.freeze(Object.assign(Object.create(null), globalThis.__WS_ENV));
  const event = Object.freeze(Object.assign(Object.create(null), globalThis.__WS_EVENT));
  const stringify = value => typeof value === "string" ? value : JSON.stringify(value);
  globalThis.ws = Object.freeze({
    event,
    environment,
    send(value) { sends.push(stringify(value)); },
    sendJson(value) { sends.push(JSON.stringify(value)); },
    log(...values) { logs.push(values.map(stringify).join(" ")); },
  });
  globalThis.__WS_FINISH = () => ({ sends, logs });
})();
"#;

pub async fn run_websocket_connection(
    url: &str,
    headers: &[HeaderEntry],
    mut commands: UnboundedReceiver<WebSocketCommand>,
    signals: UnboundedSender<WebSocketSignal>,
) -> Result<(), WebSocketConnectError> {
    if let Err(error) = crate::tls::install_crypto_provider() {
        let _ = signals.send(WebSocketSignal::Failed(error.to_owned()));
        return Ok(());
    }
    let parsed = url::Url::parse(url)
        .map_err(|error| WebSocketConnectError::InvalidUrl(error.to_string()))?;
    if !matches!(parsed.scheme(), "ws" | "wss") {
        return Err(WebSocketConnectError::UnsupportedScheme);
    }
    let mut request = url
        .into_client_request()
        .map_err(|error| WebSocketConnectError::InvalidUrl(error.to_string()))?;
    for header in headers
        .iter()
        .filter(|header| header.enabled && !header.name.trim().is_empty())
    {
        let name = HeaderName::from_bytes(header.name.trim().as_bytes()).map_err(|error| {
            WebSocketConnectError::InvalidHeaderName {
                name: header.name.clone(),
                reason: error.to_string(),
            }
        })?;
        let value = HeaderValue::from_str(&header.value).map_err(|error| {
            WebSocketConnectError::InvalidHeaderValue {
                name: header.name.clone(),
                reason: error.to_string(),
            }
        })?;
        request.headers_mut().append(name, value);
    }
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_WEBSOCKET_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_WEBSOCKET_MESSAGE_BYTES));
    let (stream, _) = match connect_async_with_config(request, Some(config), true).await {
        Ok(connection) => connection,
        Err(error) => {
            let _ = signals.send(WebSocketSignal::Failed(error.to_string()));
            return Ok(());
        }
    };
    if signals.send(WebSocketSignal::Connected).is_err() {
        return Ok(());
    }
    let (mut writer, mut reader) = stream.split();
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { return Ok(()); };
                let message = match command {
                    WebSocketCommand::SendText(text) => Message::Text(text.into()),
                    WebSocketCommand::SendBinary(bytes) => Message::Binary(bytes.into()),
                    WebSocketCommand::Close => Message::Close(None),
                };
                if let Err(error) = writer.send(message).await {
                    let _ = signals.send(WebSocketSignal::Failed(error.to_string()));
                    return Ok(());
                }
            }
            incoming = reader.next() => {
                let Some(incoming) = incoming else {
                    let _ = signals.send(WebSocketSignal::Closed(None));
                    return Ok(());
                };
                let signal = match incoming {
                    Ok(Message::Text(text)) => WebSocketSignal::Text(text.to_string()),
                    Ok(Message::Binary(bytes)) => WebSocketSignal::Binary(bytes.to_vec()),
                    Ok(Message::Ping(bytes)) => WebSocketSignal::Ping(bytes.to_vec()),
                    Ok(Message::Pong(bytes)) => WebSocketSignal::Pong(bytes.to_vec()),
                    Ok(Message::Close(frame)) => {
                        let reason = frame.map(|frame| format!("{} {}", u16::from(frame.code), frame.reason));
                        let _ = signals.send(WebSocketSignal::Closed(reason));
                        return Ok(());
                    }
                    Ok(Message::Frame(_)) => continue,
                    Err(error) => {
                        let _ = signals.send(WebSocketSignal::Failed(error.to_string()));
                        return Ok(());
                    }
                };
                if signals.send(signal).is_err() { return Ok(()); }
            }
        }
    }
}

pub async fn replay_websocket_frames(
    frames: Vec<WebSocketReplayFrame>,
    sender: UnboundedSender<WebSocketCommand>,
) {
    for frame in frames {
        if frame.delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(frame.delay_ms)).await;
        }
        if sender
            .send(WebSocketCommand::SendText(frame.payload))
            .is_err()
        {
            return;
        }
    }
}

pub fn binary_preview(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        text.to_owned()
    } else {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::net::TcpListener;
    use tokio_tungstenite::{
        accept_hdr_async,
        tungstenite::handshake::server::{Request, Response},
    };

    use super::*;

    #[test]
    fn templates_use_distinct_percent_markers_and_repeat_values() {
        let source = r#"{"room":"%{room}%","again":"%{ room }%","token":"{{token}}"}"#;
        assert_eq!(template_variable_names(source).unwrap(), vec!["room"]);
        let values = BTreeMap::from([("room".to_owned(), "support".to_owned())]);
        assert_eq!(
            render_message_template(source, &values).unwrap(),
            r#"{"room":"support","again":"support","token":"{{token}}"}"#
        );
    }

    #[test]
    fn templates_reject_unclosed_and_missing_values() {
        assert_eq!(
            template_variable_names("%{room").unwrap_err(),
            WebSocketDocumentError::UnclosedTemplateVariable(0)
        );
        assert_eq!(
            render_message_template("%{room}%", &BTreeMap::new()).unwrap_err(),
            WebSocketDocumentError::MissingTemplateValue("room".to_owned())
        );
    }

    #[test]
    fn automation_can_react_to_received_messages() {
        let output = execute_websocket_automation(
            r#"if (ws.event.eventType === "message") { ws.sendJson({ ack: JSON.parse(ws.event.data).id }); ws.log("acked"); }"#,
            &WebSocketAutomationEvent::text(r#"{"id":7}"#),
            &BTreeMap::new(),
        ).unwrap();
        assert_eq!(output.sends, vec![r#"{"ack":7}"#]);
        assert_eq!(output.logs, vec!["acked"]);
    }

    #[test]
    fn replay_preserves_inter_message_delays() {
        let start = Instant::now();
        let frames = replay_frames(&[
            (start, "one".to_owned()),
            (start + Duration::from_millis(42), "two".to_owned()),
        ]);
        assert_eq!(frames[0].delay_ms, 0);
        assert_eq!(frames[1].delay_ms, 42);
    }

    #[tokio::test]
    #[allow(clippy::result_large_err)]
    async fn connection_sends_headers_and_exchanges_text_frames() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let authorization = Arc::new(Mutex::new(None));
        let captured = Arc::clone(&authorization);
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket =
                accept_hdr_async(stream, move |request: &Request, response: Response| {
                    *captured.lock().unwrap() = request
                        .headers()
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        .map(ToOwned::to_owned);
                    Ok(response)
                })
                .await
                .unwrap();
            let message = socket.next().await.unwrap().unwrap();
            socket.send(message).await.unwrap();
        });
        let (command_sender, command_receiver) = tokio::sync::mpsc::unbounded_channel();
        let (signal_sender, mut signal_receiver) = tokio::sync::mpsc::unbounded_channel();
        let url = format!("ws://{address}/events");
        let client = tokio::spawn(async move {
            run_websocket_connection(
                &url,
                &[HeaderEntry::new("Authorization", "Bearer test")],
                command_receiver,
                signal_sender,
            )
            .await
            .unwrap();
        });

        assert_eq!(
            signal_receiver.recv().await,
            Some(WebSocketSignal::Connected)
        );
        command_sender
            .send(WebSocketCommand::SendText("hello".to_owned()))
            .unwrap();
        assert_eq!(
            signal_receiver.recv().await,
            Some(WebSocketSignal::Text("hello".to_owned()))
        );
        server.await.unwrap();
        client.await.unwrap();
        assert_eq!(
            authorization.lock().unwrap().as_deref(),
            Some("Bearer test")
        );
    }
}
