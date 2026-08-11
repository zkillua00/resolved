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
    ConnectionLost,
    Change(RealtimeResourceChange),
    AuthenticationRequired,
    Unavailable,
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

        if sender.send(RealtimeSignal::ConnectionLost).is_err() {
            return Ok(());
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
    use std::sync::{Arc, Mutex};

    use tokio::net::TcpListener;
    use tokio_tungstenite::{
        accept_async, accept_hdr_async,
        tungstenite::{
            Message,
            handshake::server::{Request, Response},
        },
    };

    use super::*;

    async fn next_signal(
        receiver: &mut tokio::sync::mpsc::UnboundedReceiver<RealtimeSignal>,
    ) -> RealtimeSignal {
        tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .expect("timed out waiting for a real-time signal")
            .expect("the real-time watcher stopped unexpectedly")
    }

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

    #[tokio::test]
    #[allow(clippy::result_large_err)]
    async fn authenticates_and_receives_resource_changes() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let authorization = Arc::new(Mutex::new(None));
        let received_authorization = Arc::clone(&authorization);
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let callback = move |request: &Request, response: Response| {
                *received_authorization.lock().unwrap() = request
                    .headers()
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .map(ToOwned::to_owned);
                Ok(response)
            };
            let mut socket = accept_hdr_async(stream, callback).await.unwrap();
            socket
                .send(Message::Text(
                    r#"{
                        "command":"resource.changed",
                        "data":{
                            "event_id":"event-live",
                            "resource":"workspace",
                            "action":"updated",
                            "resource_id":"workspace-live",
                            "workspace_id":"workspace-live",
                            "occurred_at":"2026-08-11T10:00:00Z"
                        }
                    }"#
                    .into(),
                ))
                .await
                .unwrap();
        });
        let base_url = Url::parse(&format!("http://{address}/")).unwrap();
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let watcher = tokio::spawn(async move {
            watch_upstream_changes(
                &base_url,
                "session-token",
                Utc::now() + chrono::Duration::minutes(1),
                sender,
            )
            .await
        });

        assert_eq!(next_signal(&mut receiver).await, RealtimeSignal::Connected);
        let RealtimeSignal::Change(change) = next_signal(&mut receiver).await else {
            panic!("expected a resource change");
        };
        assert_eq!(change.event_id, "event-live");
        assert_eq!(change.workspace_id.as_deref(), Some("workspace-live"));
        assert_eq!(
            authorization.lock().unwrap().as_deref(),
            Some("Bearer session-token")
        );

        watcher.abort();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn reconnects_after_a_connection_closes() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut first_socket = accept_async(stream).await.unwrap();
            first_socket.close(None).await.unwrap();

            let (stream, _) = listener.accept().await.unwrap();
            let mut second_socket = accept_async(stream).await.unwrap();
            second_socket
                .send(Message::Text(
                    r#"{
                        "command":"resource.changed",
                        "data":{
                            "event_id":"event-after-reconnect",
                            "resource":"collection",
                            "action":"created",
                            "resource_id":"collection-live",
                            "workspace_id":"workspace-live",
                            "collection_id":"collection-live",
                            "occurred_at":"2026-08-11T10:01:00Z"
                        }
                    }"#
                    .into(),
                ))
                .await
                .unwrap();
        });
        let base_url = Url::parse(&format!("http://{address}/")).unwrap();
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let watcher = tokio::spawn(async move {
            watch_upstream_changes(
                &base_url,
                "session-token",
                Utc::now() + chrono::Duration::minutes(1),
                sender,
            )
            .await
        });

        assert_eq!(next_signal(&mut receiver).await, RealtimeSignal::Connected);
        assert_eq!(
            next_signal(&mut receiver).await,
            RealtimeSignal::ConnectionLost
        );
        assert_eq!(next_signal(&mut receiver).await, RealtimeSignal::Connected);
        let RealtimeSignal::Change(change) = next_signal(&mut receiver).await else {
            panic!("expected a resource change after reconnecting");
        };
        assert_eq!(change.event_id, "event-after-reconnect");

        watcher.abort();
        server.await.unwrap();
    }
}
