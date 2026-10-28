use std::future::Future;
use std::time::Duration;

use bytes::Bytes;
use futures_util::stream::FuturesUnordered;
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use serde::Deserialize;
use serde_json::value::RawValue;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_tungstenite::tungstenite::{Error as WebSocketError, Message};
use tracing::{error, info, warn};
use trogon_nats::client::RequestClient;
use trogon_nats::jetstream::{ClaimCheckPublisher, JetStreamPublisher, ObjectStorePut};

use super::config::{SlackConfig, SlackSocketModeConfig};
use super::connections_open_error_code::ConnectionsOpenErrorCode;
use super::constants::{
    CLIENT_PING_PAYLOAD, DISCONNECT_REASON_LINK_DISABLED, ENVELOPE_CONCURRENCY_LIMIT, RECONNECT_INITIAL_DELAY,
    RECONNECT_MAX_DELAY, SOCKET_LIVENESS,
};
use super::delivery_metadata::{DeliveryContext, ReplyAcceptance};
use super::reply::Acknowledgement;
use super::server::SlackBridge;
use super::socket_liveness::{PongWatch, SocketLiveness};
use super::unroutable::UnroutableReason;

#[derive(Debug, thiserror::Error)]
pub enum SocketModeError {
    #[error("slack socket_mode config is missing")]
    MissingSocketModeConfig,
    #[error("Slack Socket Mode HTTP request failed: {0}")]
    Http(#[source] reqwest::Error),
    #[error("Slack apps.connections.open failed: {0}")]
    Api(ConnectionsOpenErrorCode),
    #[error("Slack disabled Socket Mode for this app")]
    LinkDisabled,
    #[error("Slack Socket Mode WebSocket failed: {0}")]
    WebSocket(#[source] tokio_tungstenite::tungstenite::Error),
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

impl SocketModeError {
    pub fn is_permanent(&self) -> bool {
        match self {
            Self::Api(code) => code.is_permanent(),
            Self::LinkDisabled => true,
            Self::MissingSocketModeConfig | Self::Http(_) | Self::WebSocket(_) => false,
        }
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
    payload: Option<Box<RawValue>>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    retry_attempt: Option<serde_json::Value>,
    #[serde(default)]
    retry_reason: Option<String>,
    #[serde(default)]
    accepts_response_payload: Option<bool>,
}

pub trait SocketConnector<P: JetStreamPublisher, S: ObjectStorePut, R: RequestClient> {
    fn connect<'a>(
        &'a mut self,
        bridge: &'a SlackBridge<P, S, R>,
        config: &'a SlackSocketModeConfig,
    ) -> impl std::future::Future<Output = Result<(), SocketModeError>> + Send + 'a;
}

pub struct HttpSocketConnector {
    client: reqwest::Client,
    open_url: String,
}

impl HttpSocketConnector {
    pub fn new(open_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            open_url: open_url.into(),
        }
    }
}

impl<P: JetStreamPublisher, S: ObjectStorePut, R: RequestClient> SocketConnector<P, S, R> for HttpSocketConnector {
    async fn connect<'a>(
        &'a mut self,
        bridge: &'a SlackBridge<P, S, R>,
        config: &'a SlackSocketModeConfig,
    ) -> Result<(), SocketModeError> {
        connect_once(&self.client, &self.open_url, bridge, config).await
    }
}

pub async fn run<P: JetStreamPublisher, S: ObjectStorePut, R: RequestClient>(
    publisher: ClaimCheckPublisher<P, S>,
    requester: R,
    config: SlackConfig,
    mut connector: impl SocketConnector<P, S, R>,
) -> Result<(), SocketModeError> {
    let socket_mode = config
        .socket_mode()
        .ok_or(SocketModeError::MissingSocketModeConfig)?
        .clone();
    let bridge = SlackBridge::new(publisher, requester, &config);
    let mut reconnect_delay = RECONNECT_INITIAL_DELAY;

    loop {
        match connector.connect(&bridge, &socket_mode).await {
            Ok(()) => reconnect_delay = RECONNECT_INITIAL_DELAY,
            Err(error) if error.is_permanent() => {
                error!(error = %error, "Slack Socket Mode connection can never succeed until the app is fixed");
            }
            Err(error) => warn!(error = %error, "Slack Socket Mode connection failed"),
        }

        tokio::time::sleep(reconnect_delay).await;
        reconnect_delay = next_reconnect_delay(reconnect_delay);
    }
}

fn next_reconnect_delay(current: Duration) -> Duration {
    current.saturating_mul(2).min(RECONNECT_MAX_DELAY)
}

async fn connect_once<P: JetStreamPublisher, S: ObjectStorePut, R: RequestClient>(
    client: &reqwest::Client,
    open_url: &str,
    bridge: &SlackBridge<P, S, R>,
    config: &SlackSocketModeConfig,
) -> Result<(), SocketModeError> {
    let websocket_url = open_socket_url(client, open_url, config).await?;
    info!("connecting to Slack Socket Mode");
    let (ws, _) = tokio_tungstenite::connect_async(&websocket_url).await?;
    let (mut sender, mut receiver) = ws.split();
    process_socket_messages(bridge, &mut receiver, &mut sender, SOCKET_LIVENESS).await
}

async fn process_socket_messages<P, S, R, Incoming, Outgoing>(
    bridge: &SlackBridge<P, S, R>,
    receiver: &mut Incoming,
    sender: &mut Outgoing,
    liveness: SocketLiveness,
) -> Result<(), SocketModeError>
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
    R: RequestClient,
    Incoming: Stream<Item = Result<Message, WebSocketError>> + Unpin,
    Outgoing: Sink<Message, Error = WebSocketError> + Unpin,
{
    let ping_interval = liveness.ping_interval();
    let mut pings = tokio::time::interval_at(Instant::now() + ping_interval, ping_interval);
    pings.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut pong_watch = PongWatch::new(liveness);
    let server_ping_timeout = liveness.server_ping_timeout();
    let server_ping_deadline = tokio::time::sleep(server_ping_timeout);
    tokio::pin!(server_ping_deadline);
    let mut server_ping_seen = false;
    let mut in_flight = FuturesUnordered::new();

    loop {
        let stall_deadline = pong_watch.stall_deadline();
        tokio::select! {
            biased;
            _ = pings.tick() => {
                sender.send(Message::Ping(Bytes::from_static(CLIENT_PING_PAYLOAD))).await?;
                pong_watch.ping_sent(Instant::now());
            }
            Some(ack) = in_flight.next(), if !in_flight.is_empty() => {
                if let Some(ack) = ack {
                    sender.send(ack_message(ack)).await?;
                }
            }
            message = receiver.next(), if in_flight.len() < ENVELOPE_CONCURRENCY_LIMIT => {
                let Some(message) = message else {
                    drain_acks(&mut in_flight, sender).await;
                    return Ok(());
                };
                match message? {
                    Message::Text(text) => match inspect_frame(text.as_str()) {
                        Ok(Frame::Ignored) => {}
                        Ok(Frame::Reconnect) => {
                            drain_acks(&mut in_flight, sender).await;
                            return Ok(());
                        }
                        Ok(Frame::Delivery(envelope)) => {
                            in_flight.push(async move { deliver_envelope(bridge, envelope, text.as_str()).await });
                        }
                        Err(error) => {
                            drain_acks(&mut in_flight, sender).await;
                            return Err(error);
                        }
                    },
                    Message::Close(_) => {
                        drain_acks(&mut in_flight, sender).await;
                        return Ok(());
                    }
                    Message::Ping(payload) => {
                        server_ping_deadline.as_mut().reset(Instant::now() + server_ping_timeout);
                        server_ping_seen = true;
                        sender.send(Message::Pong(payload)).await?;
                    }
                    Message::Pong(_) => pong_watch.pong_received(),
                    Message::Binary(_) | Message::Frame(_) => {}
                }
            }
            () = tokio::time::sleep_until(stall_deadline.unwrap_or_else(Instant::now)), if stall_deadline.is_some() => {
                let timeout = liveness.client_ping_timeout();
                warn!(timeout = ?timeout, "Slack Socket Mode pong was not received in time, reconnecting");
                return Ok(());
            }
            () = &mut server_ping_deadline, if server_ping_seen => {
                warn!(timeout = ?server_ping_timeout, "Slack Socket Mode ping was not received in time, reconnecting");
                return Ok(());
            }
        }
    }
}

fn ack_message(ack: String) -> Message {
    Message::Text(ack.into())
}

async fn drain_acks<Delivery, Outgoing>(in_flight: &mut FuturesUnordered<Delivery>, sender: &mut Outgoing)
where
    Delivery: Future<Output = Option<String>>,
    Outgoing: Sink<Message, Error = WebSocketError> + Unpin,
{
    while let Some(ack) = in_flight.next().await {
        if let Some(ack) = ack
            && sender.send(ack_message(ack)).await.is_err()
        {
            return;
        }
    }
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
            .ok_or_else(|| SocketModeError::Api(ConnectionsOpenErrorCode::new("missing url")));
    }

    Err(SocketModeError::Api(ConnectionsOpenErrorCode::new(
        response.error.unwrap_or_else(|| "unknown error".to_string()),
    )))
}

enum Frame {
    Ignored,
    Reconnect,
    Delivery(SocketEnvelope),
}

fn inspect_frame(text: &str) -> Result<Frame, SocketModeError> {
    let envelope = match serde_json::from_str::<SocketEnvelope>(text) {
        Ok(envelope) => envelope,
        Err(parse_error) => {
            let category = parse_error.classify();
            let line = parse_error.line();
            let column = parse_error.column();
            warn!(?category, line, column, "Ignoring malformed Slack Socket Mode frame");
            return Ok(Frame::Ignored);
        }
    };

    match envelope.kind.as_str() {
        "hello" => {
            info!("Slack Socket Mode connection established");
            Ok(Frame::Ignored)
        }
        "disconnect" => {
            let reason = envelope.reason.as_deref().unwrap_or("unknown");
            warn!(reason, "Slack Socket Mode disconnect requested");
            if reason == DISCONNECT_REASON_LINK_DISABLED {
                return Err(SocketModeError::LinkDisabled);
            }
            Ok(Frame::Reconnect)
        }
        _ => Ok(Frame::Delivery(envelope)),
    }
}

async fn deliver_envelope<P: JetStreamPublisher, S: ObjectStorePut, R: RequestClient>(
    bridge: &SlackBridge<P, S, R>,
    envelope: SocketEnvelope,
    text: &str,
) -> Option<String> {
    match envelope.kind.as_str() {
        "events_api" | "interactive" | "slash_commands" => handle_payload_envelope(bridge, envelope, text).await,
        other => {
            warn!(kind = other, "Unhandled Slack Socket Mode envelope type");
            let stored = bridge
                .publish_socket_unroutable(
                    UnroutableReason::UnhandledSocketModeType,
                    Bytes::copy_from_slice(text.as_bytes()),
                )
                .await;
            envelope
                .envelope_id
                .filter(|_| stored.is_success())
                .map(|envelope_id| ack_frame(envelope_id, None))
        }
    }
}

#[cfg(test)]
pub(super) async fn handle_text_frame<P: JetStreamPublisher, S: ObjectStorePut, R: RequestClient>(
    bridge: &SlackBridge<P, S, R>,
    text: &str,
) -> Result<(bool, Option<String>), SocketModeError> {
    match inspect_frame(text)? {
        Frame::Ignored => Ok((false, None)),
        Frame::Reconnect => Ok((true, None)),
        Frame::Delivery(envelope) => Ok((false, deliver_envelope(bridge, envelope, text).await)),
    }
}

async fn handle_payload_envelope<P: JetStreamPublisher, S: ObjectStorePut, R: RequestClient>(
    bridge: &SlackBridge<P, S, R>,
    envelope: SocketEnvelope,
    raw_text: &str,
) -> Option<String> {
    let Some(envelope_id) = envelope.envelope_id else {
        warn!(kind = envelope.kind, "Missing Slack Socket Mode envelope_id");
        bridge
            .publish_socket_unroutable(
                UnroutableReason::MissingEnvelopeId,
                Bytes::copy_from_slice(raw_text.as_bytes()),
            )
            .await;
        return None;
    };

    let Some(payload) = envelope.payload else {
        warn!(kind = envelope.kind, "Missing Slack Socket Mode payload");
        bridge
            .publish_socket_unroutable(
                UnroutableReason::MissingSocketModePayload,
                Bytes::copy_from_slice(raw_text.as_bytes()),
            )
            .await;
        return None;
    };

    let reply = match envelope.accepts_response_payload {
        Some(false) => ReplyAcceptance::Refused,
        _ => ReplyAcceptance::Accepted,
    };
    let Ok(context) = DeliveryContext::socket(
        &envelope_id,
        envelope.retry_attempt.as_ref().and_then(serde_json::Value::as_i64),
        envelope.retry_reason.as_deref(),
    )
    .map(|context| context.with_reply_acceptance(reply)) else {
        warn!(
            kind = envelope.kind,
            "Slack Socket Mode delivery metadata is not valid header text"
        );
        bridge
            .publish_socket_unroutable(
                UnroutableReason::InvalidHeaderValue,
                Bytes::copy_from_slice(payload.get().as_bytes()),
            )
            .await;
        return None;
    };

    let acknowledgement = match envelope.kind.as_str() {
        "events_api" => {
            let body = Bytes::copy_from_slice(payload.get().as_bytes());
            bridge.handle_json_body(body, &context).await.0.into()
        }
        "interactive" => bridge.handle_socket_interaction(&payload, &context).await,
        "slash_commands" => bridge.handle_socket_slash_command(&payload, &context).await,
        other => {
            warn!(kind = other, "Unhandled Slack Socket Mode payload envelope type");
            bridge
                .publish_socket_unroutable(
                    UnroutableReason::UnhandledSocketModeType,
                    Bytes::copy_from_slice(raw_text.as_bytes()),
                )
                .await;
            return None;
        }
    };

    match acknowledgement {
        Acknowledgement::Status(status) if status.is_success() => Some(ack_frame(envelope_id, None)),
        Acknowledgement::Status(_) => None,
        Acknowledgement::Reply(reply) => {
            let payload = reply.socket_payload();
            if payload.is_none() {
                warn!("Consumer reply is not valid JSON, acknowledging without a payload");
            }
            Some(ack_frame(envelope_id, payload))
        }
    }
}

/// The acknowledgement Slack expects back on the socket for a delivered
/// envelope.
#[derive(serde::Serialize)]
struct AckFrame {
    envelope_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    payload: Option<Box<RawValue>>,
}

fn ack_frame(envelope_id: String, payload: Option<Box<RawValue>>) -> String {
    serde_json::to_string(&AckFrame { envelope_id, payload }).unwrap_or_default()
}

#[cfg(test)]
mod tests;
