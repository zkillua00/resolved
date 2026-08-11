use std::time::Duration;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::{SinkExt as _, StreamExt as _};
use serde::Deserialize;
use thiserror::Error;
use tokio::sync::mpsc::UnboundedSender;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{
        Error as WebSocketError, Message,
        client::IntoClientRequest as _,
        http::{HeaderValue, header},
        protocol::WebSocketConfig,
    },
};
use url::Url;

const REALTIME_PATH: &str = "api/v1/ws";
const MAX_EVENT_BYTES: usize = 128 * 1024;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(25);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct RealtimeResourceChange {
    pub event_id: String,
    pub resource: String,
    pub action: String,
    pub resource_id: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub collection_id: Option<String>,
    #[serde(default)]
    pub environment_id: Option<String>,
    pub occurred_at: DateTime<Utc>,
}

impl RealtimeResourceChange {
    pub fn affects_workspace(&self, workspace_id: &str) -> bool {
        self.workspace_id.as_deref() == Some(workspace_id)
    }

    pub fn is_identity_change(&self) -> bool {
        matches!(self.resource.as_str(), "user" | "role")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RealtimeSignal {
    Connected,
    Change(RealtimeResourceChange),
    AuthenticationRequired,
}

#[derive(Debug, Error)]
pub enum RealtimeUrlError {
    #[error("the server URL cannot be used for real-time updates")]
    UnsupportedScheme,
    #[error("the server URL cannot be joined with the real-time endpoint: {0}")]
    InvalidEndpoint(#[from] url::ParseError),
}

#[derive(Deserialize)]
struct RealtimeEnvelope {
    command: String,
    data: RealtimeResourceChange,
}

enum ConnectionEnd {
    Unavailable,
    Disconnected,
    AuthenticationRequired,
    ReceiverClosed,
}

pub async fn watch_upstream_changes(
    base_url: &Url,
    bearer_token: &str,
    expires_at: DateTime<Utc>,
    sender: UnboundedSender<RealtimeSignal>,
) -> Result<(), RealtimeUrlError> {
    let websocket_url = realtime_url(base_url)?;
    let mut reconnect_delay = Duration::from_secs(1);

    loop {
        if sender.is_closed() {
            return Ok(());
        }
        if expires_at <= Utc::now() {
            let _ = sender.send(RealtimeSignal::AuthenticationRequired);
            return Ok(());
        }

        match run_connection(&websocket_url, bearer_token, &sender).await {
            ConnectionEnd::AuthenticationRequired => {
                let _ = sender.send(RealtimeSignal::AuthenticationRequired);
                return Ok(());
            }
            ConnectionEnd::ReceiverClosed => return Ok(()),
            ConnectionEnd::Disconnected => reconnect_delay = Duration::from_secs(1),
            ConnectionEnd::Unavailable => {}
        }

        tokio::time::sleep(reconnect_delay).await;
        reconnect_delay = reconnect_delay.saturating_mul(2).min(MAX_RECONNECT_DELAY);
    }
}

async fn run_connection(
    websocket_url: &Url,
    bearer_token: &str,
    sender: &UnboundedSender<RealtimeSignal>,
) -> ConnectionEnd {
    let mut request = match websocket_url.as_str().into_client_request() {
        Ok(request) => request,
        Err(error) => {
            tracing::warn!(%error, "could not create the real-time request");
            return ConnectionEnd::Unavailable;
        }
    };
    let authorization = match HeaderValue::from_str(&format!("Bearer {bearer_token}")) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, "could not authorize the real-time request");
            return ConnectionEnd::AuthenticationRequired;
        }
    };
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, authorization);
    request.headers_mut().insert(
        header::USER_AGENT,
        HeaderValue::from_static(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION")
        )),
    );

    let config = WebSocketConfig::default()
        .read_buffer_size(16 * 1024)
        .write_buffer_size(4 * 1024)
        .max_message_size(Some(MAX_EVENT_BYTES))
        .max_frame_size(Some(MAX_EVENT_BYTES));
    let (stream, _) = match connect_async_with_config(request, Some(config), true).await {
        Ok(connection) => connection,
        Err(WebSocketError::Http(response)) if matches!(response.status().as_u16(), 401 | 403) => {
            return ConnectionEnd::AuthenticationRequired;
        }
        Err(error) => {
            tracing::debug!(%error, "real-time connection unavailable");
            return ConnectionEnd::Unavailable;
        }
    };

    if sender.send(RealtimeSignal::Connected).is_err() {
        return ConnectionEnd::ReceiverClosed;
    }

    let (mut writer, mut reader) = stream.split();
    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;

    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if let Err(error) = writer.send(Message::Ping(Bytes::new())).await {
                    tracing::debug!(%error, "real-time heartbeat failed");
                    return ConnectionEnd::Disconnected;
                }
            }
            message = reader.next() => {
                let Some(message) = message else {
                    return ConnectionEnd::Disconnected;
                };
                let message = match message {
                    Ok(message) => message,
                    Err(error) => {
                        tracing::debug!(%error, "real-time connection closed");
                        return ConnectionEnd::Disconnected;
                    }
                };
                if message.is_close() {
                    return ConnectionEnd::Disconnected;
                }
                if !message.is_text() && !message.is_binary() {
                    continue;
                }
                let Ok(envelope) = serde_json::from_slice::<RealtimeEnvelope>(&message.into_data()) else {
                    continue;
                };
                if envelope.command != "resource.changed" {
                    continue;
                }
                if sender.send(RealtimeSignal::Change(envelope.data)).is_err() {
                    return ConnectionEnd::ReceiverClosed;
                }
            }
        }
    }
}

fn realtime_url(base_url: &Url) -> Result<Url, RealtimeUrlError> {
    let mut url = base_url.join(REALTIME_PATH)?;
    let scheme = match url.scheme() {
        "http" => "ws",
        "https" => "wss",
        _ => return Err(RealtimeUrlError::UnsupportedScheme),
    };
    url.set_scheme(scheme)
        .map_err(|_| RealtimeUrlError::UnsupportedScheme)?;
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_realtime_url_with_server_base_path() {
        let secure = Url::parse("https://resolved.example.com/team/").unwrap();
        assert_eq!(
            realtime_url(&secure).unwrap().as_str(),
            "wss://resolved.example.com/team/api/v1/ws"
        );

        let loopback = Url::parse("http://127.0.0.1:8787/").unwrap();
        assert_eq!(
            realtime_url(&loopback).unwrap().as_str(),
            "ws://127.0.0.1:8787/api/v1/ws"
        );
    }

    #[test]
    fn parses_resource_change_envelope() {
        let envelope: RealtimeEnvelope = serde_json::from_str(
            r#"{
                "command":"resource.changed",
                "data":{
                    "event_id":"event-1",
                    "resource":"request",
                    "action":"updated",
                    "resource_id":"request-1",
                    "workspace_id":"workspace-1",
                    "collection_id":"collection-1",
                    "occurred_at":"2026-08-11T10:00:00Z"
                }
            }"#,
        )
        .unwrap();

        assert_eq!(envelope.command, "resource.changed");
        assert_eq!(envelope.data.resource, "request");
        assert!(envelope.data.affects_workspace("workspace-1"));
        assert!(!envelope.data.is_identity_change());
    }
}
