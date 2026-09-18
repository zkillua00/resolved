//! First-class WebSocket request documents and bounded runtime helpers.
//!
//! The persisted document is deliberately separate from the live socket. It
//! records reusable intent (messages, templates, automation and replays), while
//! the UI owns the ephemeral connection and wire timeline.

use std::{collections::BTreeMap, time::Duration};

use futures::{SinkExt as _, StreamExt as _};
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

use super::execution_diagnostics::{
    ErrorEnvelope, ResponseContext, ServerError, bounded_text, describe_body, invalid_response,
    json_error_reason, transport_failure,
};
use super::{HeaderEntry, RawBodyLanguage, execution_limits::ExecutionLimits};

pub const MAX_WEBSOCKET_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_WEBSOCKET_TIMELINE_ENTRIES: usize = 2_000;

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
    Reconnect(WebSocketReconnectOptions),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebSocketSignal {
    Reconnecting(WebSocketReconnectOptions),
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
    #[serde(skip_serializing_if = "Option::is_none")]
    collection_id: Option<String>,
    url: String,
    /// The saved request being executed, when known, so the server can apply
    /// request- and collection-scoped proxies.
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
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
    #[serde(default)]
    request_id: String,
    #[serde(default)]
    error: Option<ServerError>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WebSocketAutomationEvent {
    pub event_type: String,
    pub data: Option<String>,
    pub binary_base64: Option<String>,
    pub reason: Option<String>,
    pub error: Option<String>,
}

impl WebSocketAutomationEvent {
    pub fn closed(reason: Option<String>, error: Option<String>) -> Self {
        Self {
            event_type: "close".into(),
            data: None,
            binary_base64: None,
            reason,
            error,
        }
    }

    pub fn opened() -> Self {
        Self {
            event_type: "open".to_owned(),
            data: None,
            binary_base64: None,
            reason: None,
            error: None,
        }
    }

    pub fn text(data: impl Into<String>) -> Self {
        Self {
            event_type: "message".to_owned(),
            data: Some(data.into()),
            binary_base64: None,
            reason: None,
            error: None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WebSocketAutomationOutput {
    #[serde(default)]
    pub sends: Vec<String>,
    #[serde(default)]
    pub environment_mutations: Vec<super::script::EnvironmentMutation>,
    #[serde(default)]
    pub logs: Vec<String>,
    #[serde(default)]
    pub reconnect: Option<WebSocketReconnectOptions>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebSocketReconnectOptions {
    #[serde(default)]
    pub clear_console: bool,
    #[serde(default)]
    pub delay_ms: u64,
    #[serde(default)]
    pub url: Option<String>,
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
    #[error("invalid execution limit: {0}")]
    InvalidLimit(String),
    #[error("WebSocket handshake timed out")]
    HandshakeTimeout,
    #[error("{0}")]
    ProxyOpening(String),
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

#[cfg(test)]
pub fn execute_websocket_automation_with_modules(
    source: &str,
    modules: &BTreeMap<String, String>,
    event: &WebSocketAutomationEvent,
    environment: &BTreeMap<String, String>,
) -> Result<WebSocketAutomationOutput, String> {
    super::script::execute_websocket_script(
        source,
        modules,
        event,
        &super::RequestDraft::default(),
        &super::script::ScriptScope {
            environment: super::script::ScriptEnvironment::new(environment.clone()),
            ..Default::default()
        },
        &Default::default(),
        None,
    )
}

pub(super) fn validate_automation_sources(
    source: &str,
    modules: &BTreeMap<String, String>,
) -> Result<(), String> {
    if modules.len() > 64
        || source
            .len()
            .saturating_add(modules.values().map(String::len).sum::<usize>())
            > 1024 * 1024
    {
        return Err("Automation supports up to 64 modules and 1 MiB of source.".into());
    }
    for name in modules.keys() {
        validate_automation_module_name(name)?;
    }
    Ok(())
}

pub(super) const AUTOMATION_PRELUDE: &str = r#"
(() => {
  "use strict";
  const sends = [];
  const handlers = [];
  let reconnect = null;

  const environment = Object.freeze(api.environment.toObject());
  const event = Object.freeze(Object.assign(Object.create(null), globalThis.__WS_EVENT));
  const stringify = value => typeof value === "string" ? value : JSON.stringify(value);
  const send = value => {
    if (event.eventType === "close") throw new Error("Cannot send during a close event; send from an open handler after reconnecting");
    sends.push(value);
  };
  globalThis.ws = Object.freeze({
    event,
    environment,
    send(value) { send(stringify(value)); },
    sendJson(value) { send(JSON.stringify(value)); },
    reconnect(options = {}) {
      if (event.eventType !== "close") throw new Error("ws.reconnect() is only available in close events");
      if (options === null || typeof options !== "object" || Array.isArray(options)) {
        throw new TypeError("ws.reconnect() expects an options object");
      }
      for (const key of Object.keys(options)) {
        if (!["clearConsole", "delayMs", "url"].includes(key)) throw new TypeError("Unknown reconnect option: " + key);
      }
      const { clearConsole = false, delayMs = 1000, url = null } = options;
      if (typeof clearConsole !== "boolean") throw new TypeError("clearConsole must be a boolean");
      if (!Number.isSafeInteger(delayMs) || delayMs < 0 || delayMs > 86400000) {
        throw new TypeError("delayMs must be an integer between 0 and 86400000");
      }
      if (url !== null && (typeof url !== "string" || !/^wss?:\/\//.test(url))) {
        throw new TypeError("url must be an absolute ws:// or wss:// URL");
      }
      reconnect = { clearConsole, delayMs, url };
    },
    log: console.log,
  });
  globalThis.eventTypes = Object.freeze({
    open: (ws, event) => event.eventType === "open",
    message: (ws, event) => event.eventType === "message",
    close: (ws, event) => event.eventType === "close",
  });
  globalThis.on = (condition, handler) => {
    if (typeof condition !== "function" || typeof handler !== "function") {
      throw new TypeError("on(condition, handler) requires two functions");
    }
    handlers.push([condition, handler]);
  };
  globalThis.__WS_DISPATCH = async () => {
    for (const [condition, handler] of handlers) {
      if (await condition(ws, event)) await handler(ws, event);
    }
  };
  globalThis.__WS_FINISH = () => ({ sends, reconnect });
})();
"#;

#[cfg(test)]
pub async fn run_websocket_session(
    url: &str,
    headers: &[HeaderEntry],
    commands: UnboundedReceiver<WebSocketCommand>,
    signals: UnboundedSender<WebSocketSignal>,
) -> Result<(), WebSocketConnectError> {
    run_websocket_session_with_limits(url, headers, commands, signals, &ExecutionLimits::default())
        .await
}

pub async fn run_websocket_session_with_limits(
    url: &str,
    headers: &[HeaderEntry],
    commands: UnboundedReceiver<WebSocketCommand>,
    signals: UnboundedSender<WebSocketSignal>,
    limits: &ExecutionLimits,
) -> Result<(), WebSocketConnectError> {
    limits
        .validate()
        .map_err(WebSocketConnectError::InvalidLimit)?;
    run_reconnecting_session(url, headers, commands, signals, None, limits).await;
    Ok(())
}

pub async fn run_upstream_websocket_session_with_scope(
    base_url: &url::Url,
    bearer_token: &str,
    workspace_id: &str,
    saved_request_id: Option<&str>,
    url: &str,
    headers: &[HeaderEntry],
    commands: UnboundedReceiver<WebSocketCommand>,
    signals: UnboundedSender<WebSocketSignal>,
    collection_id: Option<&str>,
    limits: &ExecutionLimits,
) -> Result<(), WebSocketConnectError> {
    limits
        .validate()
        .map_err(WebSocketConnectError::InvalidLimit)?;
    run_reconnecting_session(
        url,
        headers,
        commands,
        signals,
        Some(UpstreamWebSocketTarget {
            base_url: base_url.clone(),
            bearer_token: bearer_token.to_owned(),
            workspace_id: workspace_id.to_owned(),
            saved_request_id: saved_request_id.map(ToOwned::to_owned),
            collection_id: collection_id.map(str::to_owned),
        }),
        limits,
    )
    .await;
    Ok(())
}

#[derive(Clone)]
struct UpstreamWebSocketTarget {
    base_url: url::Url,
    bearer_token: String,
    workspace_id: String,
    saved_request_id: Option<String>,
    collection_id: Option<String>,
}

type ConnectionFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), WebSocketConnectError>> + Send>>;

// Keep the session command channel alive after transport closure. Dropping this
// future cancels both the current transport and any pending reconnect delay.
async fn run_reconnecting_session(
    url: &str,
    headers: &[HeaderEntry],
    mut commands: UnboundedReceiver<WebSocketCommand>,
    signals: UnboundedSender<WebSocketSignal>,
    upstream: Option<UpstreamWebSocketTarget>,
    limits: &ExecutionLimits,
) {
    let mut current_url = url.to_owned();
    let (wire_sender, mut wire_receiver) = tokio::sync::mpsc::unbounded_channel();
    let connect = |url: String| {
        let headers = headers.to_vec();
        let upstream = upstream.clone();
        let limits = limits.clone();
        let signals = wire_sender.clone();
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let future: ConnectionFuture = Box::pin(async move {
            match upstream {
                Some(target) => {
                    run_upstream_websocket_connection_with_scope(
                        &target.base_url,
                        &target.bearer_token,
                        &target.workspace_id,
                        target.saved_request_id.as_deref(),
                        &url,
                        &headers,
                        receiver,
                        signals,
                        target.collection_id.as_deref(),
                        &limits,
                    )
                    .await
                }
                None => {
                    run_websocket_connection_with_limits(&url, &headers, receiver, signals, &limits)
                        .await
                }
            }
        });
        (sender, future)
    };
    let (mut sender, mut connection) = connect(current_url.clone());
    let mut running = true;
    let mut deadline = None;
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                None => return,
                Some(WebSocketCommand::Close) => {
                    let _ = sender.send(WebSocketCommand::Close);
                    if running {
                        let _ = tokio::time::timeout(Duration::from_secs(1), &mut connection).await;
                    }
                    let mut closed = false;
                    while let Ok(signal) = wire_receiver.try_recv() {
                        closed |= matches!(signal, WebSocketSignal::Closed(_) | WebSocketSignal::Failed(_));
                        let _ = signals.send(signal);
                    }
                    if !closed { let _ = signals.send(WebSocketSignal::Closed(None)); }
                    return;
                }
                Some(WebSocketCommand::Reconnect(options)) => {
                    let next_url = options.url.as_deref().unwrap_or(&current_url);
                    if !url::Url::parse(next_url).is_ok_and(|url| matches!(url.scheme(), "ws" | "wss") && url.host_str().is_some())
                        || options.delay_ms > 86_400_000
                    {
                        let _ = signals.send(WebSocketSignal::Failed("Invalid reconnect URL or delay".into()));
                        continue;
                    }
                    current_url = next_url.to_owned();
                    // Replace (and drop) the old transport before starting the delay.
                    (sender, connection) = connect(current_url.clone());
                    running = false;
                    while wire_receiver.try_recv().is_ok() {}
                    deadline = Some(tokio::time::Instant::now() + Duration::from_millis(options.delay_ms));
                    if signals.send(WebSocketSignal::Reconnecting(options)).is_err() { return; }
                }
                Some(command) => { if running { let _ = sender.send(command); } }
            },
            result = &mut connection, if running => {
                running = false;
                if let Err(error) = result {
                    let _ = signals.send(WebSocketSignal::Failed(error.to_string()));
                }
            }
            signal = wire_receiver.recv() => {
                if let Some(signal) = signal {
                    if signals.send(signal).is_err() { return; }
                }
            }
            _ = async {
                match deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                deadline = None;
                running = true;
            }
        }
    }
}

#[cfg(test)]
pub async fn run_websocket_connection(
    url: &str,
    headers: &[HeaderEntry],
    commands: UnboundedReceiver<WebSocketCommand>,
    signals: UnboundedSender<WebSocketSignal>,
) -> Result<(), WebSocketConnectError> {
    run_websocket_connection_with_limits(
        url,
        headers,
        commands,
        signals,
        &ExecutionLimits::default(),
    )
    .await
}

pub async fn run_websocket_connection_with_limits(
    url: &str,
    headers: &[HeaderEntry],
    commands: UnboundedReceiver<WebSocketCommand>,
    signals: UnboundedSender<WebSocketSignal>,
    limits: &ExecutionLimits,
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
    let message_limit = limits
        .get("websocket.message_bytes")
        .as_usize()
        .map_err(WebSocketConnectError::InvalidLimit)?;
    let timeout = limits
        .get("websocket.handshake_timeout_ms")
        .as_duration()
        .map_err(WebSocketConnectError::InvalidLimit)?;
    let config = WebSocketConfig::default()
        .max_message_size(message_limit)
        .max_frame_size(message_limit);
    let (stream, _) = match handshake_timeout(
        timeout,
        connect_async_with_config(request, Some(config), true),
    )
    .await?
    {
        Ok(connection) => connection,
        Err(error) => {
            let _ = signals.send(WebSocketSignal::Failed(error.to_string()));
            return Ok(());
        }
    };
    drive_websocket_connection(stream, commands, signals, message_limit).await;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub async fn run_upstream_websocket_connection(
    base_url: &url::Url,
    bearer_token: &str,
    workspace_id: &str,
    saved_request_id: Option<&str>,
    url: &str,
    headers: &[HeaderEntry],
    commands: UnboundedReceiver<WebSocketCommand>,
    signals: UnboundedSender<WebSocketSignal>,
) -> Result<(), WebSocketConnectError> {
    run_upstream_websocket_connection_with_scope(
        base_url,
        bearer_token,
        workspace_id,
        saved_request_id,
        url,
        headers,
        commands,
        signals,
        None,
        &ExecutionLimits::default(),
    )
    .await
}

pub async fn run_upstream_websocket_connection_with_scope(
    base_url: &url::Url,
    bearer_token: &str,
    workspace_id: &str,
    saved_request_id: Option<&str>,
    url: &str,
    headers: &[HeaderEntry],
    commands: UnboundedReceiver<WebSocketCommand>,
    signals: UnboundedSender<WebSocketSignal>,
    collection_id: Option<&str>,
    limits: &ExecutionLimits,
) -> Result<(), WebSocketConnectError> {
    if let Err(error) = crate::tls::install_crypto_provider() {
        let _ = signals.send(WebSocketSignal::Failed(error.to_owned()));
        return Ok(());
    }
    let mut endpoint = base_url
        .join(&format!("api/v1/workspaces/{workspace_id}/execute"))
        .map_err(|error| WebSocketConnectError::InvalidUrl(error.to_string()))?;
    if let Some(id) = collection_id {
        endpoint.query_pairs_mut().append_pair("collection_id", id);
    }
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
    let message_limit = limits
        .get("websocket.message_bytes")
        .as_usize()
        .map_err(WebSocketConnectError::InvalidLimit)?;
    let opening_limit = limits
        .get("websocket.opening_bytes")
        .as_usize()
        .map_err(WebSocketConnectError::InvalidLimit)?;
    let timeout = limits
        .get("websocket.handshake_timeout_ms")
        .as_duration()
        .map_err(WebSocketConnectError::InvalidLimit)?;
    // Control messages are opening traffic, not application messages (which may have a zero budget).
    // tokio-tungstenite cannot change config after upgrading. Bound the transport
    // by the larger phase budget and enforce each phase's own budget below.
    let transport_limit = match (message_limit, opening_limit) {
        (Some(message), Some(opening)) => Some(message.max(opening)),
        _ => None,
    };
    let config = WebSocketConfig::default()
        .max_message_size(transport_limit)
        .max_frame_size(transport_limit);
    let started = std::time::Instant::now();
    let descriptor = UpstreamWebSocketOpen {
        collection_id: collection_id.map(str::to_owned),
        url: url.to_owned(),
        request_id: saved_request_id.map(ToOwned::to_owned),
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
    if opening_limit.is_some_and(|limit| payload.len() > limit) {
        return Err(WebSocketConnectError::InvalidLimit(format!(
            "WebSocket proxy opening send: descriptor is {} bytes; websocket.opening_bytes limit is {} bytes",
            payload.len(),
            opening_limit.unwrap()
        )));
    }
    let (mut stream, context) =
        match connect_proxy_websocket(request, config, timeout, started).await {
            Ok(connection) => connection,
            Err(error) => {
                let _ = signals.send(WebSocketSignal::Failed(error));
                return Ok(());
            }
        };
    if let Err(error) = proxy_opening_timeout(
        timeout.map(|budget| budget.saturating_sub(started.elapsed())),
        timeout,
        "opening send",
        stream.send(Message::Text(payload.into())),
    )
    .await?
    {
        let _ = signals.send(WebSocketSignal::Failed(format!(
            "WebSocket proxy opening send failed: {error}"
        )));
        return Ok(());
    }
    let response = match proxy_opening_timeout(
        timeout.map(|budget| budget.saturating_sub(started.elapsed())),
        timeout,
        "opening receive",
        stream.next(),
    )
    .await?
    {
        Some(Ok(Message::Text(response))) => response,
        Some(Ok(frame)) => {
            let _ = signals.send(WebSocketSignal::Failed(invalid_response(
                "WebSocket proxy opening receive",
                OPENING_EXPECTED,
                &describe_opening_frame(&frame),
                None,
            )));
            return Ok(());
        }
        Some(Err(error)) => {
            let _ = signals.send(WebSocketSignal::Failed(format!(
                "WebSocket proxy opening receive failed: {error}"
            )));
            return Ok(());
        }
        None => {
            let _ = signals.send(WebSocketSignal::Failed(invalid_response(
                "WebSocket proxy opening receive",
                OPENING_EXPECTED,
                "end of stream without an opening response",
                None,
            )));
            return Ok(());
        }
    };
    if opening_limit.is_some_and(|limit| response.len() > limit) {
        return Err(WebSocketConnectError::InvalidLimit(format!(
            "WebSocket proxy opening validation: response is {} bytes; websocket.opening_bytes limit is {} bytes",
            response.len(),
            opening_limit.unwrap()
        )));
    }
    let response: UpstreamWebSocketOpenResponse = match serde_json::from_str(&response) {
        Ok(response) => response,
        Err(error) => {
            let _ = signals.send(WebSocketSignal::Failed(invalid_response(
                "WebSocket proxy opening validation",
                OPENING_EXPECTED,
                &describe_body(response.as_bytes()),
                Some(&json_error_reason(&error)),
            )));
            return Ok(());
        }
    };
    if response.response_type == "error" {
        let mut error = response.error.unwrap_or_default();
        if error.message.trim().is_empty() {
            error.message = if response.message.trim().is_empty() {
                "server rejected the WebSocket opening without an error message".into()
            } else {
                response.message
            };
        }
        let message = context.failure("WebSocket proxy opening", &error, &response.request_id);
        let _ = signals.send(WebSocketSignal::Failed(message));
        return Ok(());
    }
    if response.response_type != "opened" {
        let _ = signals.send(WebSocketSignal::Failed(invalid_response(
            "WebSocket proxy opening validation",
            OPENING_EXPECTED,
            "JSON object with an unrecognized string in `type`",
            None,
        )));
        return Ok(());
    }
    drive_websocket_connection(stream, commands, signals, message_limit).await;
    Ok(())
}

const OPENING_EXPECTED: &str = r#"a text frame containing {"type":"opened"} or {"type":"error","message":string,"error"?:object,"request_id"?:string}"#;

fn describe_opening_frame(frame: &Message) -> String {
    match frame {
        Message::Text(text) => format!("text frame: {}", describe_body(text.as_bytes())),
        Message::Binary(bytes) => format!("binary frame ({} bytes)", bytes.len()),
        Message::Ping(bytes) => format!("ping frame ({} bytes)", bytes.len()),
        Message::Pong(bytes) => format!("pong frame ({} bytes)", bytes.len()),
        Message::Close(Some(frame)) => format!(
            "close frame (code {}, reason: {})",
            u16::from(frame.code),
            bounded_text(&frame.reason)
        ),
        Message::Close(None) => "close frame without a code or reason".into(),
        Message::Frame(_) => "raw WebSocket frame".into(),
    }
}

async fn proxy_opening_timeout<F: std::future::Future>(
    remaining: Option<Duration>,
    budget: Option<Duration>,
    phase: &str,
    future: F,
) -> Result<F::Output, WebSocketConnectError> {
    handshake_timeout(remaining, future).await.map_err(|_| {
        WebSocketConnectError::ProxyOpening(format!(
            "WebSocket proxy {phase} timed out; websocket.handshake_timeout_ms limit is {} ms (shared opening budget)",
            budget.unwrap_or_default().as_millis()
        ))
    })
}

/// Use the HTTP client's framed body reader for rejections: tungstenite retains
/// only bytes read alongside the headers, losing delayed/chunked error bodies.
/// This is still one request, and successful upgrades retain buffered wire bytes.
async fn connect_proxy_websocket(
    request: tokio_tungstenite::tungstenite::http::Request<()>,
    config: WebSocketConfig,
    timeout: Option<Duration>,
    started: std::time::Instant,
) -> Result<
    (
        tokio_tungstenite::WebSocketStream<reqwest::Upgraded>,
        ResponseContext,
    ),
    String,
> {
    use tokio_tungstenite::tungstenite::{handshake::derive_accept_key, protocol::Role};
    const OPERATION: &str = "WebSocket proxy HTTP upgrade";
    const EXPECTED: &str = "HTTP 101 Switching Protocols with a valid WebSocket upgrade";
    const MAX_ERROR_BODY: usize = 64 * 1024;

    let accept = derive_accept_key(request.headers()["sec-websocket-key"].as_bytes());
    let mut url = url::Url::parse(&request.uri().to_string())
        .map_err(|_| format!("{OPERATION}: invalid endpoint URL"))?;
    let scheme = if url.scheme() == "wss" {
        "https"
    } else {
        "http"
    };
    url.set_scheme(scheme)
        .map_err(|_| format!("{OPERATION}: invalid endpoint scheme"))?;
    let client = reqwest::Client::builder()
        .http1_only()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .build()
        .map_err(|error| transport_failure(&format!("{OPERATION} setup"), error))?;
    let mut response = proxy_opening_timeout(
        timeout.map(|budget| budget.saturating_sub(started.elapsed())),
        timeout,
        "HTTP upgrade",
        client.get(url).headers(request.headers().clone()).send(),
    )
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| transport_failure(OPERATION, error))?;
    let context = ResponseContext::from_http(response.status(), response.headers());
    if response.status() != reqwest::StatusCode::SWITCHING_PROTOCOLS {
        let mut body = Vec::new();
        let mut read_failure = None;
        loop {
            match proxy_opening_timeout(
                timeout.map(|budget| budget.saturating_sub(started.elapsed())),
                timeout,
                "HTTP rejection body read",
                response.chunk(),
            )
            .await
            {
                Ok(Ok(Some(chunk))) => {
                    let remaining = MAX_ERROR_BODY - body.len();
                    body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                    if chunk.len() > remaining {
                        read_failure = Some(format!(
                            "rejection body exceeds {MAX_ERROR_BODY} byte diagnostic limit"
                        ));
                        break;
                    }
                }
                Ok(Ok(None)) => break,
                Ok(Err(error)) => {
                    read_failure = Some(transport_failure("rejection body read", error));
                    break;
                }
                Err(error) => {
                    read_failure = Some(format!("{error}; received {} body bytes", body.len()));
                    break;
                }
            }
        }
        if let Ok(envelope) = serde_json::from_slice::<ErrorEnvelope>(&body)
            && let Some(error) = envelope.error
        {
            let mut message = context.failure(OPERATION, &error, &envelope.request_id);
            if let Some(failure) = read_failure {
                message.push_str(&format!("; {failure}"));
            }
            return Err(message);
        }
        return Err(invalid_response(
            OPERATION,
            EXPECTED,
            &format!("{}; {}", context.describe(), describe_body(&body)),
            read_failure.as_deref(),
        ));
    }
    let headers = response.headers();
    let has_token = |name: &str, token: &str| {
        headers
            .get_all(name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .any(|value| value.trim().eq_ignore_ascii_case(token))
    };
    let mut invalid_headers = Vec::new();
    if !has_token("connection", "upgrade") {
        invalid_headers.push("Connection header missing required Upgrade token");
    }
    if !has_token("upgrade", "websocket") {
        invalid_headers.push("Upgrade header missing required websocket token");
    }
    if headers.get_all("sec-websocket-accept").iter().count() != 1 {
        invalid_headers.push("expected exactly one Sec-WebSocket-Accept header");
    } else if headers
        .get("sec-websocket-accept")
        .map(|value| value.as_bytes())
        != Some(accept.as_bytes())
    {
        invalid_headers.push("Sec-WebSocket-Accept accept-key mismatch");
    }
    if headers.contains_key("sec-websocket-protocol") {
        invalid_headers.push("unsolicited Sec-WebSocket-Protocol");
    }
    if headers.contains_key("sec-websocket-extensions") {
        invalid_headers.push("unsolicited Sec-WebSocket-Extensions");
    }
    if !invalid_headers.is_empty() {
        return Err(invalid_response(
            OPERATION,
            EXPECTED,
            &context.describe(),
            Some(&invalid_headers.join("; ")),
        ));
    }
    let upgraded = proxy_opening_timeout(
        timeout.map(|budget| budget.saturating_sub(started.elapsed())),
        timeout,
        "HTTP upgrade transport setup",
        response.upgrade(),
    )
    .await
    .map_err(|error| format!("{error} ({})", context.describe()))?
    .map_err(|error| transport_failure(&format!("{OPERATION} transport setup"), error))?;
    Ok((
        tokio_tungstenite::WebSocketStream::from_raw_socket(upgraded, Role::Client, Some(config))
            .await,
        context,
    ))
}

async fn handshake_timeout<F: std::future::Future>(
    duration: Option<Duration>,
    future: F,
) -> Result<F::Output, WebSocketConnectError> {
    match duration {
        Some(duration) if duration.is_zero() => Err(WebSocketConnectError::HandshakeTimeout),
        Some(duration) => tokio::time::timeout(duration, future)
            .await
            .map_err(|_| WebSocketConnectError::HandshakeTimeout),
        None => Ok(future.await),
    }
}

async fn drive_websocket_connection<S>(
    stream: tokio_tungstenite::WebSocketStream<S>,
    mut commands: UnboundedReceiver<WebSocketCommand>,
    signals: UnboundedSender<WebSocketSignal>,
    message_limit: Option<usize>,
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
                    WebSocketCommand::Reconnect(_) => return,
                };
                if matches!(message, Message::Text(_) | Message::Binary(_))
                    && message_limit.is_some_and(|limit| message.len() > limit) {
                    let _ = signals.send(WebSocketSignal::Failed("WebSocket message exceeds message_bytes".into()));
                    continue;
                }
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
                if let Ok(message @ (Message::Text(_) | Message::Binary(_))) = &incoming
                    && message_limit.is_some_and(|limit| message.len() > limit) {
                    let _ = signals.send(WebSocketSignal::Failed("WebSocket message exceeds message_bytes".into()));
                    return;
                }
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
    use std::time::Instant;

    #[test]
    fn scoped_opening_descriptor_and_zero_timeout() {
        let descriptor = UpstreamWebSocketOpen {
            collection_id: Some("nested-collection".into()),
            request_id: Some("saved-request".into()),
            url: "wss://example.com".into(),
            headers: vec![],
        };
        let encoded = serde_json::to_value(descriptor).unwrap();
        assert_eq!(encoded["collection_id"], "nested-collection");
        assert_eq!(encoded["request_id"], "saved-request");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert!(
            runtime
                .block_on(handshake_timeout(Some(Duration::ZERO), async {}))
                .is_err()
        );
        assert_eq!(
            runtime
                .block_on(handshake_timeout(None, async { 7 }))
                .unwrap(),
            7
        );
    }

    use tokio::net::TcpListener;
    use tokio_tungstenite::{
        accept_hdr_async,
        tungstenite::handshake::server::{Request, Response},
    };

    use super::*;

    async fn proxy_failure_from_frame(frame: Message) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = url::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            assert!(matches!(socket.next().await, Some(Ok(Message::Text(_)))));
            socket.send(frame).await.unwrap();
        });
        let (_commands, receiver) = tokio::sync::mpsc::unbounded_channel();
        let (signals, mut received) = tokio::sync::mpsc::unbounded_channel();
        run_upstream_websocket_connection(
            &base,
            "test-token",
            "workspace",
            None,
            "wss://example.test/",
            &[],
            receiver,
            signals,
        )
        .await
        .unwrap();
        server.await.unwrap();
        match received.recv().await.unwrap() {
            WebSocketSignal::Failed(message) => message,
            other => panic!("expected failure, received {other:?}"),
        }
    }

    #[tokio::test]
    async fn proxy_opening_errors_preserve_structured_and_legacy_details() {
        let error = proxy_failure_from_frame(Message::Text(r#"{"type":"error","message":"legacy","request_id":"opening-id","error":{"code":"validation_failed","message":"Invalid descriptor","fields":{"url":"Unsupported scheme"},"phase":"validation","reason":"bad_url"}}"#.into())).await;
        for expected in [
            "Invalid descriptor",
            "validation_failed",
            "url",
            "Unsupported scheme",
            "validation",
            "bad_url",
            "opening-id",
        ] {
            assert!(error.contains(expected), "{expected}: {error}");
        }
        let error = proxy_failure_from_frame(Message::Text(
            r#"{"type":"error","message":"Legacy refusal"}"#.into(),
        ))
        .await;
        assert!(error.contains("Legacy refusal"), "{error}");
    }

    #[tokio::test]
    async fn proxy_opening_invalid_frames_are_actionable_and_redacted() {
        for (frame, expected) in [
            (
                Message::Text(r#"{"message":"secret-payload"}"#.into()),
                "type",
            ),
            (Message::Text(r#"{"type":42}"#.into()), "string"),
            (
                Message::Text(r#"{"type":"secret-payload"}"#.into()),
                "unrecognized",
            ),
            (Message::Text(r#"{"type":"opened""#.into()), "line"),
            (
                Message::Binary(vec![1, 2, 3].into()),
                "binary frame (3 bytes)",
            ),
            (Message::Ping(vec![1].into()), "ping frame"),
            (Message::Pong(vec![1].into()), "pong frame"),
            (Message::Close(None), "close frame"),
            (
                Message::Close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
                    code:
                        tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Policy,
                    reason: "policy refusal".into(),
                })),
                "1008",
            ),
        ] {
            let error = proxy_failure_from_frame(frame).await;
            assert!(error.contains(expected), "{expected}: {error}");
            assert!(error.contains("opened"), "{error}");
            assert!(!error.contains("secret-payload"), "{error}");
        }
    }

    #[tokio::test]
    async fn proxy_upgrade_rejects_invalid_accept_without_exposing_header_values() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        crate::tls::install_crypto_provider().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.unwrap());
            }
            socket.write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: sensitive-invalid-value\r\n\r\n").await.unwrap();
        });
        let error = match connect_proxy_websocket(
            endpoint.into_client_request().unwrap(),
            WebSocketConfig::default(),
            Some(Duration::from_secs(2)),
            std::time::Instant::now(),
        )
        .await
        {
            Err(error) => error,
            Ok(_) => panic!("invalid accept key was accepted"),
        };
        server.await.unwrap();
        assert!(error.contains("101"), "{error}");
        assert!(error.contains("accept-key mismatch"), "{error}");
        assert!(!error.contains("sensitive-invalid-value"), "{error}");
    }

    #[tokio::test]
    async fn proxy_upgrade_rejects_duplicate_accept_headers() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        crate::tls::install_crypto_provider().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let request = format!("ws://{}/", listener.local_addr().unwrap())
            .into_client_request()
            .unwrap();
        let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(
            request.headers()["sec-websocket-key"].as_bytes(),
        );
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.unwrap());
            }
            socket.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {accept}\r\nSec-WebSocket-Accept: private-value\r\n\r\n").as_bytes()).await.unwrap();
        });
        let error = connect_proxy_websocket(
            request,
            WebSocketConfig::default(),
            Some(Duration::from_secs(2)),
            std::time::Instant::now(),
        )
        .await
        .err()
        .expect("duplicate accept headers must be rejected");
        server.await.unwrap();
        assert!(
            error.contains("exactly one Sec-WebSocket-Accept"),
            "{error}"
        );
        assert!(!error.contains("private-value"), "{error}");
    }

    #[tokio::test]
    async fn proxy_rejection_read_timeout_keeps_already_received_http_details() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        crate::tls::install_crypto_provider().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.unwrap());
            }
            socket.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nX-Request-ID: stalled-rejection\r\nContent-Length: 100\r\n\r\n{}").await.unwrap();
            // Keep the body incomplete until the client's deadline closes it.
            let _ = socket.read_u8().await;
        });
        let error = connect_proxy_websocket(
            endpoint.into_client_request().unwrap(),
            WebSocketConfig::default(),
            Some(Duration::from_millis(250)),
            std::time::Instant::now(),
        )
        .await
        .err()
        .expect("stalled rejection must time out");
        server.await.unwrap();
        for detail in [
            "HTTP 403",
            "application/json",
            "stalled-rejection",
            "HTTP rejection body read timed out",
            "250 ms",
            "received 2 body bytes",
        ] {
            assert!(error.contains(detail), "{detail}: {error}");
        }
    }

    #[tokio::test]
    async fn proxy_upgrade_rejection_reads_delayed_chunked_envelope() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = url::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.unwrap());
            }
            socket.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nX-Request-ID: header-id\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
            tokio::time::sleep(Duration::from_millis(30)).await;
            let body = r#"{"success":false,"request_id":"body-id","error":{"code":"forbidden","message":"Workspace access denied","fields":{"workspace":"Not accessible"}}}"#;
            socket
                .write_all(format!("{:x}\r\n{body}\r\n0\r\n\r\n", body.len()).as_bytes())
                .await
                .unwrap();
        });
        let (_commands, receiver) = tokio::sync::mpsc::unbounded_channel();
        let (signals, mut received) = tokio::sync::mpsc::unbounded_channel();
        run_upstream_websocket_connection(
            &base,
            "test-token",
            "workspace",
            None,
            "wss://example.test/",
            &[],
            receiver,
            signals,
        )
        .await
        .unwrap();
        server.await.unwrap();
        let WebSocketSignal::Failed(error) = received.recv().await.unwrap() else {
            panic!("expected rejection")
        };
        for expected in [
            "HTTP upgrade",
            "403",
            "application/json",
            "body-id",
            "forbidden",
            "Workspace access denied",
            "Not accessible",
        ] {
            assert!(error.contains(expected), "{expected}: {error}");
        }
    }

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
    fn automation_handlers_filter_events_and_await_in_registration_order() {
        let modules = BTreeMap::from([(
            "handlers.js".into(),
            r#"
            on(eventTypes.open, async (ws, event) => {
                await Promise.resolve();
                ws.send(event.eventType);
            });
            on(eventTypes.message, (ws, event) => ws.send(event.binaryBase64 ?? event.data));
        "#
            .into(),
        )]);
        let source = r#"
            import './handlers.js';
            let ready = false;
            on(async (api, event) => {
                if (api !== ws || event !== ws.event) throw new Error('wrong arguments');
                return await Promise.resolve(eventTypes.message(api, event));
            }, async (ws, event) => {
                await Promise.resolve();
                ready = true;
                ws.send('first');
            });
            on(() => ready, ws => ws.send('second'));
            on(() => false, () => { throw new Error('must not run'); });
        "#;
        for (event, expected) in [
            (WebSocketAutomationEvent::opened(), vec!["open"]),
            (
                WebSocketAutomationEvent::text("hello"),
                vec!["hello", "first", "second"],
            ),
            (
                WebSocketAutomationEvent {
                    event_type: "message".into(),
                    data: None,
                    binary_base64: Some("AA==".into()),
                    reason: None,
                    error: None,
                },
                vec!["AA==", "first", "second"],
            ),
        ] {
            let output = execute_websocket_automation_with_modules(
                source,
                &modules,
                &event,
                &BTreeMap::new(),
            )
            .unwrap();
            assert_eq!(output.sends, expected);
        }
    }

    #[test]
    fn automation_handler_errors_fail_execution() {
        for source in [
            "on(null, () => {});",
            "on(eventTypes.open, null);",
            "on(() => { throw new Error('predicate failure'); }, () => {});",
            "on(async () => { await Promise.resolve(); throw new Error('predicate failure'); }, () => {});",
            "on(eventTypes.open, async ws => { ws.send('discard'); await Promise.resolve(); throw new Error('handler failure'); });",
        ] {
            let error = execute_websocket_automation(
                source,
                &WebSocketAutomationEvent::opened(),
                &BTreeMap::new(),
            )
            .unwrap_err();
            assert!(
                error.contains("requires two functions") || error.contains("failure"),
                "{error}"
            );
        }
    }

    #[test]
    fn automation_close_can_schedule_reconnection() {
        let output = execute_websocket_automation(
            r#"on(eventTypes.close, async (ws, event) => {
                ws.log(event.reason, event.error);
                await Promise.resolve();
                ws.reconnect();
                ws.reconnect({ clearConsole: true, delayMs: 25, url: 'wss://other.example/socket' });
            });"#,
            &WebSocketAutomationEvent::closed(Some("1000 finished".into()), None),
            &BTreeMap::new(),
        ).unwrap();
        assert_eq!(output.logs, vec!["1000 finished null"]);
        assert_eq!(
            output.reconnect,
            Some(WebSocketReconnectOptions {
                clear_console: true,
                delay_ms: 25,
                url: Some("wss://other.example/socket".into()),
            })
        );
        let output = execute_websocket_automation(
            "on(eventTypes.close, ws => ws.reconnect());",
            &WebSocketAutomationEvent::closed(None, Some("reset".into())),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(output.reconnect.unwrap().delay_ms, 1000);
        for source in [
            "ws.reconnect({ delayMs: -1 });",
            "ws.reconnect({ delayMs: 1.5 });",
            "ws.reconnect({ delayMs: 86400001 });",
            "ws.reconnect({ clearConsole: 'yes' });",
            "ws.reconnect({ url: 'https://example.com' });",
            "ws.reconnect({ url: 'ws://' });",
            "ws.reconnect({ typo: true });",
            "ws.reconnect(null);",
            "ws.reconnect(); throw new Error('discard reconnect');",
        ] {
            assert!(
                execute_websocket_automation(
                    source,
                    &WebSocketAutomationEvent::closed(None, None),
                    &BTreeMap::new()
                )
                .is_err(),
                "{source}"
            );
        }
        assert!(
            execute_websocket_automation(
                "ws.reconnect();",
                &WebSocketAutomationEvent::opened(),
                &BTreeMap::new()
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn session_reconnects_after_close_with_url_override_and_can_cancel_delay() {
        let first = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let second = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let first_url = format!("ws://{}/first", first.local_addr().unwrap());
        let second_url = format!("ws://{}/second", second.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (stream, _) = first.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            socket.close(None).await.unwrap();
            let (stream, _) = second.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            socket
                .send(Message::Text("reconnected".into()))
                .await
                .unwrap();
            socket.close(None).await.unwrap();
        });
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let (signals, mut events) = tokio::sync::mpsc::unbounded_channel();
        let client = tokio::spawn(async move {
            run_websocket_session(&first_url, &[], receiver, signals)
                .await
                .unwrap();
        });
        let test = async {
            assert_eq!(events.recv().await, Some(WebSocketSignal::Connected));
            assert!(matches!(
                events.recv().await,
                Some(WebSocketSignal::Closed(_))
            ));
            let options = WebSocketReconnectOptions {
                clear_console: true,
                delay_ms: 30,
                url: Some(second_url),
            };
            let started = Instant::now();
            sender
                .send(WebSocketCommand::Reconnect(options.clone()))
                .unwrap();
            assert_eq!(
                events.recv().await,
                Some(WebSocketSignal::Reconnecting(options))
            );
            assert_eq!(events.recv().await, Some(WebSocketSignal::Connected));
            assert!(started.elapsed() >= Duration::from_millis(30));
            assert_eq!(
                events.recv().await,
                Some(WebSocketSignal::Text("reconnected".into()))
            );
            assert!(matches!(
                events.recv().await,
                Some(WebSocketSignal::Closed(_))
            ));
            sender
                .send(WebSocketCommand::Reconnect(WebSocketReconnectOptions {
                    delay_ms: 60000,
                    ..Default::default()
                }))
                .unwrap();
            assert!(matches!(
                events.recv().await,
                Some(WebSocketSignal::Reconnecting(_))
            ));
            sender.send(WebSocketCommand::Close).unwrap();
            client.await.unwrap();
            server.await.unwrap();
        };
        tokio::time::timeout(Duration::from_secs(5), test)
            .await
            .unwrap();
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
            assert_eq!(descriptor["request_id"], "request-1");
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
                Some("request-1"),
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
