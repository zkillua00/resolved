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
use rquickjs::{
    CatchResultExt as _, Context as JsContext, Module, Runtime as JsRuntime,
    loader::{BuiltinLoader, BuiltinResolver},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{
        Message,
        client::IntoClientRequest as _,
        http::{HeaderName, HeaderValue, header::AUTHORIZATION},
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
    pub automation_modules: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct WebSocketSavedMessage {
    pub id: String,
    pub name: String,
    pub payload: String,
    #[serde(default)]
    pub language: RawBodyLanguage,
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
    #[serde(default)]
    pub direction: WebSocketReplayDirection,
    /// Binary payloads are always base64, never a lossy display preview.
    #[serde(default)]
    pub binary: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WebSocketReplayDirection {
    #[default]
    Sent,
    Received,
}

impl WebSocketReplayFrame {
    pub fn command(&self) -> Result<Option<WebSocketCommand>, String> {
        if self.direction == WebSocketReplayDirection::Received {
            return Ok(None);
        }
        if self.binary {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD
                .decode(&self.payload)
                .map(|bytes| Some(WebSocketCommand::SendBinary(bytes)))
                .map_err(|error| format!("Invalid recorded binary frame: {error}"))
        } else {
            Ok(Some(WebSocketCommand::SendText(self.payload.clone())))
        }
    }
}

/// Compare each direction independently: response latency may change interleaving.
/// Payloads and frame types are exact; timing is displayed, not treated as a failure.
pub fn compare_replay_frames<'a>(
    saved: &'a [WebSocketReplayFrame],
    actual: &'a [WebSocketReplayFrame],
    direction: WebSocketReplayDirection,
) -> Vec<(
    Option<&'a WebSocketReplayFrame>,
    Option<&'a WebSocketReplayFrame>,
)> {
    let saved: Vec<_> = saved
        .iter()
        .filter(|frame| frame.direction == direction)
        .collect();
    let actual: Vec<_> = actual
        .iter()
        .filter(|frame| frame.direction == direction)
        .collect();
    (0..saved.len().max(actual.len()))
        .map(|index| (saved.get(index).copied(), actual.get(index).copied()))
        .collect()
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

#[derive(Serialize)]
struct UpstreamWebSocketOpen {
    url: String,
    headers: Vec<UpstreamWebSocketHeader>,
}

#[derive(Serialize)]
struct UpstreamWebSocketHeader {
    name: String,
    value: String,
}

#[derive(Deserialize)]
struct UpstreamWebSocketOpenResponse {
    #[serde(rename = "type")]
    response_type: String,
    #[serde(default)]
    message: String,
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

#[cfg(test)]
fn execute_websocket_automation(
    source: &str,
    event: &WebSocketAutomationEvent,
    environment: &BTreeMap<String, String>,
) -> Result<WebSocketAutomationOutput, String> {
    execute_websocket_automation_with_modules(source, &BTreeMap::new(), event, environment)
}

pub fn validate_automation_module_name(name: &str) -> Result<(), String> {
    if name == "automation.js"
        || !name.ends_with(".js")
        || name.len() > 128
        || name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/'))
    {
        return Err("Use a relative .js filename, such as helpers.js or lib/messages.js. automation.js is the entry file.".into());
    }
    Ok(())
}

pub fn execute_websocket_automation_with_modules(
    source: &str,
    modules: &BTreeMap<String, String>,
    event: &WebSocketAutomationEvent,
    environment: &BTreeMap<String, String>,
) -> Result<WebSocketAutomationOutput, String> {
    if modules.len() > 64
        || source
            .len()
            .saturating_add(modules.values().map(String::len).sum::<usize>())
            > 1024 * 1024
    {
        return Err("Automation supports up to 64 modules and 1 MiB of source.".into());
    }
    let mut resolver = BuiltinResolver::default();
    let mut loader = BuiltinLoader::default();
    for (name, source) in modules {
        validate_automation_module_name(name)?;
        let path = name.clone();
        resolver.add_module(path.clone());
        loader.add_module(path, source.as_bytes().to_vec());
    }
    if source.trim().is_empty() {
        return Ok(WebSocketAutomationOutput::default());
    }
    let runtime = JsRuntime::new().map_err(|error| error.to_string())?;
    runtime.set_loader(resolver, loader);
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
        Module::evaluate(ctx.clone(), "automation.js", source)
            .catch(&ctx)
            .map_err(|error| error.to_string())?
            .finish::<()>()
            .catch(&ctx)
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
  globalThis.console = Object.freeze({
    log: ws.log, info: ws.log, warn: ws.log, error: ws.log, debug: ws.log,
  });
  globalThis.__WS_FINISH = () => ({ sends, logs });
})();
"#;

pub async fn run_websocket_connection(
    url: &str,
    headers: &[HeaderEntry],
    commands: UnboundedReceiver<WebSocketCommand>,
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
    drive_websocket_connection(stream, commands, signals).await;
    Ok(())
}

pub async fn run_upstream_websocket_connection(
    base_url: &url::Url,
    bearer_token: &str,
    workspace_id: &str,
    url: &str,
    headers: &[HeaderEntry],
    commands: UnboundedReceiver<WebSocketCommand>,
    signals: UnboundedSender<WebSocketSignal>,
) -> Result<(), WebSocketConnectError> {
    if let Err(error) = crate::tls::install_crypto_provider() {
        let _ = signals.send(WebSocketSignal::Failed(error.to_owned()));
        return Ok(());
    }
    let mut endpoint = base_url
        .join(&format!("api/v1/workspaces/{workspace_id}/execute"))
        .map_err(|error| WebSocketConnectError::InvalidUrl(error.to_string()))?;
    let socket_scheme = match endpoint.scheme() {
        "http" => "ws",
        "https" => "wss",
        _ => return Err(WebSocketConnectError::UnsupportedScheme),
    };
    endpoint
        .set_scheme(socket_scheme)
        .map_err(|_| WebSocketConnectError::UnsupportedScheme)?;
    let mut request = endpoint
        .as_str()
        .into_client_request()
        .map_err(|error| WebSocketConnectError::InvalidUrl(error.to_string()))?;
    request.headers_mut().insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {bearer_token}")).map_err(|error| {
            WebSocketConnectError::InvalidHeaderValue {
                name: "Authorization".to_owned(),
                reason: error.to_string(),
            }
        })?,
    );
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_WEBSOCKET_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_WEBSOCKET_MESSAGE_BYTES));
    let (mut stream, _) = match connect_async_with_config(request, Some(config), true).await {
        Ok(connection) => connection,
        Err(error) => {
            let _ = signals.send(WebSocketSignal::Failed(error.to_string()));
            return Ok(());
        }
    };
    let descriptor = UpstreamWebSocketOpen {
        url: url.to_owned(),
        headers: headers
            .iter()
            .filter(|header| header.enabled && !header.name.trim().is_empty())
            .map(|header| UpstreamWebSocketHeader {
                name: header.name.clone(),
                value: header.value.clone(),
            })
            .collect(),
    };
    let payload = serde_json::to_string(&descriptor)
        .map_err(|error| WebSocketConnectError::InvalidUrl(error.to_string()))?;
    if let Err(error) = stream.send(Message::Text(payload.into())).await {
        let _ = signals.send(WebSocketSignal::Failed(error.to_string()));
        return Ok(());
    }
    let response = match stream.next().await {
        Some(Ok(Message::Text(response))) => response,
        Some(Ok(_)) => {
            let _ = signals.send(WebSocketSignal::Failed(
                "the server returned an invalid WebSocket execution response".to_owned(),
            ));
            return Ok(());
        }
        Some(Err(error)) => {
            let _ = signals.send(WebSocketSignal::Failed(error.to_string()));
            return Ok(());
        }
        None => {
            let _ = signals.send(WebSocketSignal::Failed(
                "the server closed the WebSocket execution connection".to_owned(),
            ));
            return Ok(());
        }
    };
    let response: UpstreamWebSocketOpenResponse = match serde_json::from_str(&response) {
        Ok(response) => response,
        Err(error) => {
            let _ = signals.send(WebSocketSignal::Failed(format!(
                "the server returned an invalid WebSocket execution response: {error}"
            )));
            return Ok(());
        }
    };
    if response.response_type != "opened" {
        let message = if response.message.trim().is_empty() {
            "the server could not open the WebSocket connection".to_owned()
        } else {
            response.message
        };
        let _ = signals.send(WebSocketSignal::Failed(message));
        return Ok(());
    }
    drive_websocket_connection(stream, commands, signals).await;
    Ok(())
}

async fn drive_websocket_connection<S>(
    stream: tokio_tungstenite::WebSocketStream<S>,
    mut commands: UnboundedReceiver<WebSocketCommand>,
    signals: UnboundedSender<WebSocketSignal>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    if signals.send(WebSocketSignal::Connected).is_err() {
        return;
    }
    let (mut writer, mut reader) = stream.split();
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { return; };
                let message = match command {
                    WebSocketCommand::SendText(text) => Message::Text(text.into()),
                    WebSocketCommand::SendBinary(bytes) => Message::Binary(bytes.into()),
                    WebSocketCommand::Close => Message::Close(None),
                };
                if let Err(error) = writer.send(message).await {
                    let _ = signals.send(WebSocketSignal::Failed(error.to_string()));
                    return;
                }
            }
            incoming = reader.next() => {
                let Some(incoming) = incoming else {
                    let _ = signals.send(WebSocketSignal::Closed(None));
                    return;
                };
                let signal = match incoming {
                    Ok(Message::Text(text)) => WebSocketSignal::Text(text.to_string()),
                    Ok(Message::Binary(bytes)) => WebSocketSignal::Binary(bytes.to_vec()),
                    Ok(Message::Ping(bytes)) => WebSocketSignal::Ping(bytes.to_vec()),
                    Ok(Message::Pong(bytes)) => WebSocketSignal::Pong(bytes.to_vec()),
                    Ok(Message::Close(frame)) => {
                        let reason = frame.map(|frame| format!("{} {}", u16::from(frame.code), frame.reason));
                        let _ = signals.send(WebSocketSignal::Closed(reason));
                        return;
                    }
                    Ok(Message::Frame(_)) => continue,
                    Err(error) => {
                        let _ = signals.send(WebSocketSignal::Failed(error.to_string()));
                        return;
                    }
                };
                if signals.send(signal).is_err() { return; }
            }
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
    fn automation_imports_nested_modules_and_awaits_results() {
        let modules = BTreeMap::from([
            ("lib/ack.js".into(), "import { decorate } from './format.js'; export async function ack(id) { return decorate(await Promise.resolve(id)); }".into()),
            ("lib/format.js".into(), "export function decorate(id) { return { ack: id }; }".into()),
        ]);
        let output = execute_websocket_automation_with_modules(
            "import { ack } from './lib/ack.js'; ws.sendJson(await ack(JSON.parse(ws.event.data).id)); console.log('sent', ws.environment.label);",
            &modules, &WebSocketAutomationEvent::text(r#"{"id":7}"#), &BTreeMap::from([("label".into(), "ack".into())]),
        ).unwrap();
        assert_eq!(output.sends, vec![r#"{"ack":7}"#]);
        assert_eq!(output.logs, vec!["sent ack"]);
    }

    #[test]
    fn automation_import_errors_and_resource_limits_are_reported() {
        let event = WebSocketAutomationEvent::opened();
        let error = execute_websocket_automation_with_modules(
            "import './missing.js';",
            &BTreeMap::new(),
            &event,
            &BTreeMap::new(),
        )
        .unwrap_err();
        assert!(error.contains("missing.js"), "{error}");
        assert!(
            execute_websocket_automation_with_modules(
                "ws.send('no');",
                &BTreeMap::from([("../escape.js".into(), "".into())]),
                &event,
                &BTreeMap::new()
            )
            .is_err()
        );
        let error = execute_websocket_automation(
            "throw new Error('specific failure');",
            &event,
            &BTreeMap::new(),
        )
        .unwrap_err();
        assert!(error.contains("specific failure"), "{error}");
        assert!(execute_websocket_automation("while (true) {}", &event, &BTreeMap::new()).is_err());
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
    fn replay_legacy_frames_are_outgoing_text() {
        let frame: WebSocketReplayFrame =
            serde_json::from_str(r#"{"delay_ms":42,"payload":"hello"}"#).unwrap();
        assert_eq!(
            frame.command().unwrap(),
            Some(WebSocketCommand::SendText("hello".into()))
        );
        let mut received = frame.clone();
        received.direction = WebSocketReplayDirection::Received;
        assert_eq!(received.command().unwrap(), None);
        received.binary = true;
        received.direction = WebSocketReplayDirection::Sent;
        received.payload = "/wAB".into();
        assert_eq!(
            received.command().unwrap(),
            Some(WebSocketCommand::SendBinary(vec![255, 0, 1]))
        );
        assert_eq!(
            serde_json::from_str::<WebSocketReplayFrame>(
                &serde_json::to_string(&received).unwrap()
            )
            .unwrap(),
            received
        );
        received.payload = "invalid!".into();
        assert!(received.command().is_err());
    }

    #[test]
    fn replay_comparison_keeps_missing_extra_and_changed_responses() {
        let frame = |payload: &str, direction| WebSocketReplayFrame {
            delay_ms: 0,
            payload: payload.into(),
            direction,
            binary: false,
        };
        let saved = vec![
            frame("request", WebSocketReplayDirection::Sent),
            frame("expected", WebSocketReplayDirection::Received),
        ];
        let actual = vec![
            frame("changed", WebSocketReplayDirection::Received),
            frame("request", WebSocketReplayDirection::Sent),
            frame("extra", WebSocketReplayDirection::Received),
        ];
        let sent = compare_replay_frames(&saved, &actual, WebSocketReplayDirection::Sent);
        assert_eq!(sent[0].0, sent[0].1);
        let received = compare_replay_frames(&saved, &actual, WebSocketReplayDirection::Received);
        assert_ne!(
            received[0].0.unwrap().payload,
            received[0].1.unwrap().payload
        );
        assert!(received[1].0.is_none());
        assert!(
            compare_replay_frames(&saved, &[], WebSocketReplayDirection::Received)[0]
                .1
                .is_none()
        );
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

    #[tokio::test]
    async fn upstream_connection_opens_with_descriptor_before_relaying_frames() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(stream, |request: &Request, response: Response| {
                assert_eq!(
                    request.uri().path(),
                    "/api/v1/workspaces/workspace-1/execute"
                );
                assert_eq!(
                    request
                        .headers()
                        .get("authorization")
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer server-session")
                );
                Ok(response)
            })
            .await
            .unwrap();
            let descriptor = socket.next().await.unwrap().unwrap();
            let Message::Text(descriptor) = descriptor else {
                panic!("expected an execution descriptor");
            };
            let descriptor: serde_json::Value = serde_json::from_str(&descriptor).unwrap();
            assert_eq!(descriptor["url"], "wss://target.example/socket");
            assert_eq!(descriptor["headers"][0]["name"], "X-Target");
            assert_eq!(descriptor["headers"][0]["value"], "yes");
            socket
                .send(Message::Text(r#"{"type":"opened"}"#.into()))
                .await
                .unwrap();
            let message = socket.next().await.unwrap().unwrap();
            socket.send(message).await.unwrap();
        });

        let (command_sender, command_receiver) = tokio::sync::mpsc::unbounded_channel();
        let (signal_sender, mut signal_receiver) = tokio::sync::mpsc::unbounded_channel();
        let base_url = url::Url::parse(&format!("http://{address}/")).unwrap();
        let client = tokio::spawn(async move {
            run_upstream_websocket_connection(
                &base_url,
                "server-session",
                "workspace-1",
                "wss://target.example/socket",
                &[HeaderEntry::new("X-Target", "yes")],
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
    }
}
