use std::fmt;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use serde::Deserialize;
use tokio_tungstenite::tungstenite::{Error as WebSocketError, Message};
use tracing::{info, warn};
use trogon_nats::jetstream::{ClaimCheckPublisher, JetStreamPublisher, ObjectStorePut};

use super::config::{SlackConfig, SlackSocketModeConfig};
use super::server::SlackBridge;

const APPS_CONNECTIONS_OPEN_URL: &str = "https://slack.com/api/apps.connections.open";
const RECONNECT_INITIAL_DELAY: Duration = Duration::from_secs(1);
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum SocketModeError {
    MissingSocketModeConfig,
    Http(reqwest::Error),
    Api(String),
    WebSocket(tokio_tungstenite::tungstenite::Error),
    Json(serde_json::Error),
}

impl fmt::Display for SocketModeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSocketModeConfig => f.write_str("slack socket_mode config is missing"),
            Self::Http(error) => write!(f, "Slack Socket Mode HTTP request failed: {error}"),
            Self::Api(error) => write!(f, "Slack apps.connections.open failed: {error}"),
            Self::WebSocket(error) => write!(f, "Slack Socket Mode WebSocket failed: {error}"),
            Self::Json(error) => write!(f, "Slack Socket Mode JSON parsing failed: {error}"),
        }
    }
}

impl std::error::Error for SocketModeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Http(error) => Some(error),
            Self::WebSocket(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::MissingSocketModeConfig | Self::Api(_) => None,
        }
    }
}

impl From<reqwest::Error> for SocketModeError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error)
    }
}

impl From<tokio_tungstenite::tungstenite::Error> for SocketModeError {
    fn from(error: tokio_tungstenite::tungstenite::Error) -> Self {
        Self::WebSocket(error)
    }
}

impl From<serde_json::Error> for SocketModeError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Deserialize)]
struct OpenConnectionResponse {
    ok: bool,
    url: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct SocketEnvelope {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    envelope_id: Option<String>,
    #[serde(default)]
    payload: Option<serde_json::Value>,
    #[serde(default)]
    reason: Option<String>,
}

#[cfg(not(coverage))]
pub async fn run<P: JetStreamPublisher, S: ObjectStorePut>(
    publisher: ClaimCheckPublisher<P, S>,
    config: &SlackConfig,
) -> Result<(), SocketModeError> {
    let socket_mode = config
        .socket_mode()
        .ok_or(SocketModeError::MissingSocketModeConfig)?
        .clone();
    let bridge = SlackBridge::new(publisher, config);
    let client = reqwest::Client::new();
    let mut reconnect_delay = RECONNECT_INITIAL_DELAY;

    loop {
        match connect_once(&client, APPS_CONNECTIONS_OPEN_URL, &bridge, &socket_mode).await {
            Ok(()) => reconnect_delay = RECONNECT_INITIAL_DELAY,
            Err(error) => warn!(error = %error, "Slack Socket Mode connection failed"),
        }

        tokio::time::sleep(reconnect_delay).await;
        reconnect_delay = next_reconnect_delay(reconnect_delay);
    }
}

fn next_reconnect_delay(current: Duration) -> Duration {
    current.saturating_mul(2).min(RECONNECT_MAX_DELAY)
}

async fn connect_once<P: JetStreamPublisher, S: ObjectStorePut>(
    client: &reqwest::Client,
    open_url: &str,
    bridge: &SlackBridge<P, S>,
    config: &SlackSocketModeConfig,
) -> Result<(), SocketModeError> {
    let websocket_url = open_socket_url(client, open_url, config).await?;
    info!("connecting to Slack Socket Mode");
    let (ws, _) = tokio_tungstenite::connect_async(&websocket_url).await?;
    let (mut sender, mut receiver) = ws.split();
    process_socket_messages(bridge, &mut receiver, &mut sender).await
}

async fn process_socket_messages<P, S, Incoming, Outgoing>(
    bridge: &SlackBridge<P, S>,
    receiver: &mut Incoming,
    sender: &mut Outgoing,
) -> Result<(), SocketModeError>
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
    Incoming: Stream<Item = Result<Message, WebSocketError>> + Unpin,
    Outgoing: Sink<Message, Error = WebSocketError> + Unpin,
{
    while let Some(message) = receiver.next().await {
        match message? {
            Message::Text(text) => {
                let (reconnect, ack) = handle_text_frame(bridge, text.as_str()).await?;
                if let Some(ack) = ack {
                    sender.send(Message::Text(ack.into())).await?;
                }
                if reconnect {
                    return Ok(());
                }
            }
            Message::Close(_) => return Ok(()),
            Message::Ping(payload) => sender.send(Message::Pong(payload)).await?,
            Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }

    Ok(())
}

async fn open_socket_url(
    client: &reqwest::Client,
    open_url: &str,
    config: &SlackSocketModeConfig,
) -> Result<String, SocketModeError> {
    let response = client
        .post(open_url)
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", config.app_token.as_str()),
        )
        .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .send()
        .await?
        .error_for_status()?
        .json::<OpenConnectionResponse>()
        .await?;

    if response.ok {
        return response
            .url
            .ok_or_else(|| SocketModeError::Api("missing url".to_string()));
    }

    Err(SocketModeError::Api(
        response.error.unwrap_or_else(|| "unknown error".to_string()),
    ))
}

async fn handle_text_frame<P: JetStreamPublisher, S: ObjectStorePut>(
    bridge: &SlackBridge<P, S>,
    text: &str,
) -> Result<(bool, Option<String>), SocketModeError> {
    let envelope: SocketEnvelope = serde_json::from_str(text)?;

    match envelope.kind.as_str() {
        "hello" => {
            info!("Slack Socket Mode connection established");
            Ok((false, None))
        }
        "disconnect" => {
            let reason = envelope.reason.as_deref().unwrap_or("unknown");
            warn!(reason, "Slack Socket Mode disconnect requested");
            Ok((true, None))
        }
        "events_api" | "interactive" | "slash_commands" => handle_payload_envelope(bridge, envelope, text)
            .await
            .map(|ack| (false, ack)),
        other => {
            warn!(kind = other, "Unhandled Slack Socket Mode envelope type");
            bridge
                .publish_socket_unroutable("unhandled_socket_mode_type", Bytes::copy_from_slice(text.as_bytes()))
                .await;
            Ok((false, envelope.envelope_id.map(ack_frame)))
        }
    }
}

async fn handle_payload_envelope<P: JetStreamPublisher, S: ObjectStorePut>(
    bridge: &SlackBridge<P, S>,
    envelope: SocketEnvelope,
    raw_text: &str,
) -> Result<Option<String>, SocketModeError> {
    let Some(envelope_id) = envelope.envelope_id else {
        warn!(kind = envelope.kind, "Missing Slack Socket Mode envelope_id");
        bridge
            .publish_socket_unroutable("missing_envelope_id", Bytes::copy_from_slice(raw_text.as_bytes()))
            .await;
        return Ok(None);
    };

    let Some(payload) = envelope.payload else {
        warn!(kind = envelope.kind, "Missing Slack Socket Mode payload");
        bridge
            .publish_socket_unroutable(
                "missing_socket_mode_payload",
                Bytes::copy_from_slice(raw_text.as_bytes()),
            )
            .await;
        return Ok(None);
    };

    let status = match envelope.kind.as_str() {
        "events_api" => {
            let body = Bytes::from(serde_json::to_vec(&payload)?);
            bridge.handle_json_body(body).await.0
        }
        "interactive" => bridge.handle_socket_interaction(&payload).await?,
        "slash_commands" => bridge.handle_socket_slash_command(&payload).await?,
        other => {
            warn!(kind = other, "Unhandled Slack Socket Mode payload envelope type");
            bridge
                .publish_socket_unroutable(
                    "unhandled_socket_mode_type",
                    Bytes::copy_from_slice(raw_text.as_bytes()),
                )
                .await;
            return Ok(None);
        }
    };

    if status.is_success() {
        Ok(Some(ack_frame(envelope_id)))
    } else {
        Ok(None)
    }
}

fn ack_frame(envelope_id: String) -> String {
    serde_json::json!({ "envelope_id": envelope_id }).to_string()
}

#[cfg(test)]
mod tests;
