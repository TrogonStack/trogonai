use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::time::Duration;

use async_nats::Message;
use bytes::Bytes;
use futures::StreamExt;
use rmcp::model::{JsonRpcMessage, RequestId};
use rmcp::service::{RoleClient, RoleServer, RxJsonRpcMessage, ServiceRole, TxJsonRpcMessage};
use rmcp::transport::Transport;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tracing::{Instrument, Span, instrument};
use trogon_nats::{FlushClient, PublishClient, RequestClient, SubscribeClient, headers_with_trace_context};

use crate::{Config, McpPeerId, McpPrefix, nats};

type PendingReplies = Arc<Mutex<HashMap<RequestId, String>>>;

pub struct NatsTransport<R, N>
where
    R: ServiceRole,
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
{
    nats: N,
    prefix: McpPrefix,
    remote_peer_id: McpPeerId,
    operation_timeout: Duration,
    subscription: N::Subscription,
    inbound_rx: mpsc::Receiver<RxJsonRpcMessage<R>>,
    inbound_tx: mpsc::Sender<RxJsonRpcMessage<R>>,
    pending_replies: PendingReplies,
}

impl<N> NatsTransport<RoleClient, N>
where
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    N::SubscribeError: 'static,
{
    pub async fn for_client(
        nats: N,
        config: &Config,
        client_id: McpPeerId,
        server_id: McpPeerId,
    ) -> Result<Self, NatsTransportError> {
        let subject = nats::mcp_client::wildcards::OneClientSubject::new(config.prefix(), &client_id);
        Self::new(
            nats,
            config.prefix().clone(),
            server_id,
            config.operation_timeout(),
            subject,
        )
        .await
    }
}

impl<N> NatsTransport<RoleServer, N>
where
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    N::SubscribeError: 'static,
{
    pub async fn for_server(
        nats: N,
        config: &Config,
        server_id: McpPeerId,
        client_id: McpPeerId,
    ) -> Result<Self, NatsTransportError> {
        let subject = nats::mcp_server::wildcards::OneServerSubject::new(config.prefix(), &server_id);
        Self::new(
            nats,
            config.prefix().clone(),
            client_id,
            config.operation_timeout(),
            subject,
        )
        .await
    }
}

impl<R, N> NatsTransport<R, N>
where
    R: ServiceRole,
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    N::SubscribeError: 'static,
{
    async fn new(
        nats: N,
        prefix: McpPrefix,
        remote_peer_id: McpPeerId,
        operation_timeout: Duration,
        subject: impl nats::markers::Subscribable,
    ) -> Result<Self, NatsTransportError> {
        let subscription =
            nats.subscribe(subject.to_string())
                .await
                .map_err(|source| NatsTransportError::Subscribe {
                    source: Box::new(source),
                })?;
        let (inbound_tx, inbound_rx) = mpsc::channel(64);

        Ok(Self {
            nats,
            prefix,
            remote_peer_id,
            operation_timeout,
            subscription,
            inbound_rx,
            inbound_tx,
            pending_replies: Arc::new(Mutex::new(HashMap::new())),
        })
    }
}

impl<R, N> Transport<R> for NatsTransport<R, N>
where
    R: ServiceRole,
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    N::RequestError: 'static,
    N::PublishError: 'static,
    N::FlushError: 'static,
{
    type Error = NatsTransportError;

    #[instrument(skip(self, item), fields(mcp.nats.subject = tracing::field::Empty, mcp.nats.direction = "send"))]
    fn send(&mut self, item: TxJsonRpcMessage<R>) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let nats = self.nats.clone();
        let prefix = self.prefix.clone();
        let remote_peer_id = self.remote_peer_id.clone();
        let timeout = self.operation_timeout;
        let inbound_tx = self.inbound_tx.clone();
        let pending_replies = self.pending_replies.clone();
        let span = Span::current();

        async move {
            match item {
                JsonRpcMessage::Request(_) => {
                    let subject = subject_for_message::<R>(&prefix, &remote_peer_id, &item)?;
                    crate::telemetry::transport::record_subject(&Span::current(), &subject);
                    let payload = serialize_message(&item)?;
                    let response = tokio::time::timeout(
                        timeout,
                        nats.request_with_headers(subject.clone(), headers_with_trace_context(), payload),
                    )
                    .await
                    .map_err(|_| NatsTransportError::RequestTimedOut {
                        subject: subject.clone(),
                    })?
                    .map_err(|source| NatsTransportError::Request {
                        subject: subject.clone(),
                        source: Box::new(source),
                    })?;
                    let response = deserialize_message::<RxJsonRpcMessage<R>>(&response.payload)?;
                    inbound_tx
                        .send(response)
                        .await
                        .map_err(|_| NatsTransportError::InboundClosed)?;
                    Ok(())
                }
                JsonRpcMessage::Notification(_) => {
                    let subject = subject_for_message::<R>(&prefix, &remote_peer_id, &item)?;
                    crate::telemetry::transport::record_subject(&Span::current(), &subject);
                    publish_raw_with_timeout(&nats, &subject, serialize_message(&item)?, timeout).await
                }
                JsonRpcMessage::Response(response) => {
                    let request_id = response.id.clone();
                    let subject = reply_subject_for_response(&request_id, &pending_replies)
                        .await
                        .ok_or(NatsTransportError::MissingReplySubject)?;
                    let item: TxJsonRpcMessage<R> = JsonRpcMessage::Response(response);
                    crate::telemetry::transport::record_subject(&Span::current(), &subject);
                    publish_raw_with_timeout(&nats, &subject, serialize_message(&item)?, timeout).await?;
                    forget_reply_subject(&request_id, &pending_replies).await;
                    Ok(())
                }
                JsonRpcMessage::Error(error) => {
                    let request_id = error.id.clone();
                    let subject = reply_subject_for_response(&request_id, &pending_replies)
                        .await
                        .ok_or(NatsTransportError::MissingReplySubject)?;
                    let item: TxJsonRpcMessage<R> = JsonRpcMessage::Error(error);
                    crate::telemetry::transport::record_subject(&Span::current(), &subject);
                    publish_raw_with_timeout(&nats, &subject, serialize_message(&item)?, timeout).await?;
                    forget_reply_subject(&request_id, &pending_replies).await;
                    Ok(())
                }
            }
        }
        .instrument(span)
    }

    #[instrument(skip(self), fields(mcp.nats.subject = tracing::field::Empty, mcp.nats.direction = "receive"))]
    async fn receive(&mut self) -> Option<RxJsonRpcMessage<R>> {
        loop {
            tokio::select! {
                inbound = self.inbound_rx.recv() => {
                    if inbound.is_some() {
                        return inbound;
                    }
                }
                message = self.subscription.next() => {
                    let message = message?;
                    crate::telemetry::transport::record_subject(&Span::current(), message.subject.as_str());
                    crate::telemetry::transport::record_direction(&Span::current(), "receive");
                    match deserialize_message::<RxJsonRpcMessage<R>>(&message.payload) {
                        Ok(parsed) => {
                            remember_reply_subject::<R>(&parsed, &message, &self.pending_replies).await;
                            return Some(parsed);
                        }
                        Err(error) => {
                            tracing::warn!(error = %error, subject = %message.subject, "Failed to decode MCP NATS message");
                        }
                    }
                }
            }
        }
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

async fn publish_raw<N>(nats: &N, subject: &str, payload: Bytes) -> Result<(), NatsTransportError>
where
    N: PublishClient + FlushClient,
    N::PublishError: 'static,
    N::FlushError: 'static,
{
    nats.publish_with_headers(subject.to_string(), headers_with_trace_context(), payload)
        .await
        .map_err(|source| NatsTransportError::Publish {
            subject: subject.to_string(),
            source: Box::new(source),
        })?;
    nats.flush().await.map_err(|source| NatsTransportError::Flush {
        source: Box::new(source),
    })
}

async fn publish_raw_with_timeout<N>(
    nats: &N,
    subject: &str,
    payload: Bytes,
    timeout: Duration,
) -> Result<(), NatsTransportError>
where
    N: PublishClient + FlushClient,
    N::PublishError: 'static,
    N::FlushError: 'static,
{
    tokio::time::timeout(timeout, publish_raw(nats, subject, payload))
        .await
        .map_err(|_| NatsTransportError::PublishTimedOut {
            subject: subject.to_string(),
        })?
}

fn serialize_message<T: serde::Serialize>(message: &T) -> Result<Bytes, NatsTransportError> {
    serde_json::to_vec(message)
        .map(Bytes::from)
        .map_err(NatsTransportError::Serialize)
}

fn deserialize_message<T: serde::de::DeserializeOwned>(payload: &[u8]) -> Result<T, NatsTransportError> {
    serde_json::from_slice(payload).map_err(NatsTransportError::Deserialize)
}

async fn reply_subject_for_response(request_id: &RequestId, pending_replies: &PendingReplies) -> Option<String> {
    pending_replies.lock().await.get(request_id).cloned()
}

async fn forget_reply_subject(request_id: &RequestId, pending_replies: &PendingReplies) {
    pending_replies.lock().await.remove(request_id);
}

async fn remember_reply_subject<R: ServiceRole>(
    item: &RxJsonRpcMessage<R>,
    message: &Message,
    pending_replies: &PendingReplies,
) {
    let JsonRpcMessage::Request(request) = item else {
        return;
    };
    let Some(reply) = &message.reply else {
        return;
    };
    pending_replies
        .lock()
        .await
        .insert(request.id.clone(), reply.to_string());
}

fn subject_for_message<R: ServiceRole>(
    prefix: &McpPrefix,
    peer_id: &McpPeerId,
    item: &TxJsonRpcMessage<R>,
) -> Result<String, NatsTransportError> {
    let method = method_for_message::<R>(item)?;
    let suffix = method_suffix(&method)?;
    let role = if R::IS_CLIENT { "server" } else { "client" };
    Ok(format!("{prefix}.{role}.{peer_id}.{suffix}"))
}

fn method_for_message<R: ServiceRole>(item: &TxJsonRpcMessage<R>) -> Result<String, NatsTransportError> {
    let value = serde_json::to_value(item).map_err(NatsTransportError::Serialize)?;
    value
        .get("method")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or(NatsTransportError::MissingMethod)
}

fn method_suffix(method: &str) -> Result<&'static str, NatsTransportError> {
    match method {
        "initialize" => Ok("initialize"),
        "ping" => Ok("ping"),
        "completion/complete" => Ok("completion.complete"),
        "logging/setLevel" => Ok("logging.set_level"),
        "prompts/list" => Ok("prompts.list"),
        "prompts/get" => Ok("prompts.get"),
        "resources/list" => Ok("resources.list"),
        "resources/templates/list" => Ok("resources.templates.list"),
        "resources/read" => Ok("resources.read"),
        "resources/subscribe" => Ok("resources.subscribe"),
        "resources/unsubscribe" => Ok("resources.unsubscribe"),
        "tools/list" => Ok("tools.list"),
        "tools/call" => Ok("tools.call"),
        "tasks/get" => Ok("tasks.get"),
        "tasks/list" => Ok("tasks.list"),
        "tasks/result" => Ok("tasks.result"),
        "tasks/cancel" => Ok("tasks.cancel"),
        "notifications/cancelled" => Ok("notifications.cancelled"),
        "notifications/progress" => Ok("notifications.progress"),
        "notifications/message" => Ok("notifications.message"),
        "notifications/resources/updated" => Ok("notifications.resources.updated"),
        "notifications/resources/list_changed" => Ok("notifications.resources.list_changed"),
        "notifications/tools/list_changed" => Ok("notifications.tools.list_changed"),
        "notifications/prompts/list_changed" => Ok("notifications.prompts.list_changed"),
        "notifications/elicitation/complete" => Ok("notifications.elicitation.complete"),
        "sampling/createMessage" => Ok("sampling.create_message"),
        "roots/list" => Ok("roots.list"),
        "elicitation/create" => Ok("elicitation.create"),
        "notifications/initialized" => Ok("notifications.initialized"),
        "notifications/roots/list_changed" => Ok("notifications.roots.list_changed"),
        _ => Err(NatsTransportError::UnsupportedMethod {
            method: method.to_string(),
        }),
    }
}

#[derive(Debug)]
pub enum NatsTransportError {
    Subscribe {
        source: Box<dyn Error + Send + Sync>,
    },
    Request {
        subject: String,
        source: Box<dyn Error + Send + Sync>,
    },
    RequestTimedOut {
        subject: String,
    },
    Publish {
        subject: String,
        source: Box<dyn Error + Send + Sync>,
    },
    PublishTimedOut {
        subject: String,
    },
    Flush {
        source: Box<dyn Error + Send + Sync>,
    },
    Serialize(serde_json::Error),
    Deserialize(serde_json::Error),
    MissingMethod,
    UnsupportedMethod {
        method: String,
    },
    MissingReplySubject,
    InboundClosed,
}

impl Display for NatsTransportError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Subscribe { .. } => write!(f, "failed to subscribe to MCP NATS subject"),
            Self::Request { subject, .. } => write!(f, "failed to request MCP NATS subject {subject}"),
            Self::RequestTimedOut { subject } => write!(f, "timed out requesting MCP NATS subject {subject}"),
            Self::Publish { subject, .. } => write!(f, "failed to publish MCP NATS subject {subject}"),
            Self::PublishTimedOut { subject } => write!(f, "timed out publishing MCP NATS subject {subject}"),
            Self::Flush { .. } => write!(f, "failed to flush MCP NATS client"),
            Self::Serialize(_) => write!(f, "failed to serialize MCP JSON-RPC message"),
            Self::Deserialize(_) => write!(f, "failed to deserialize MCP JSON-RPC message"),
            Self::MissingMethod => write!(f, "MCP JSON-RPC message is missing a method"),
            Self::UnsupportedMethod { method } => write!(f, "unsupported MCP method for NATS routing: {method}"),
            Self::MissingReplySubject => write!(f, "missing reply subject for MCP response"),
            Self::InboundClosed => write!(f, "MCP NATS inbound queue is closed"),
        }
    }
}

impl Error for NatsTransportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Subscribe { source }
            | Self::Request { source, .. }
            | Self::Publish { source, .. }
            | Self::Flush { source } => Some(source.as_ref()),
            Self::Serialize(source) | Self::Deserialize(source) => Some(source),
            Self::RequestTimedOut { .. }
            | Self::PublishTimedOut { .. }
            | Self::MissingMethod
            | Self::UnsupportedMethod { .. }
            | Self::MissingReplySubject
            | Self::InboundClosed => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::MIN_TIMEOUT_SECS;
    use rmcp::model::{
        ClientJsonRpcMessage, ClientRequest, ErrorData, ListToolsRequest, PaginatedRequestParams, PingRequest,
        ServerJsonRpcMessage, ServerNotification, ServerResult,
    };
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_nats::mocks::MockError;

    fn config() -> Config {
        Config::new(
            McpPrefix::new("mcp").unwrap(),
            trogon_nats::NatsConfig {
                servers: vec!["localhost:4222".to_string()],
                auth: trogon_nats::NatsAuth::None,
            },
        )
    }

    fn timeout_config() -> Config {
        config().with_operation_timeout(Duration::from_secs(MIN_TIMEOUT_SECS))
    }

    fn message(subject: &str, payload: Vec<u8>) -> Message {
        Message {
            subject: subject.to_string().into(),
            reply: None,
            payload: payload.into(),
            headers: None,
            length: 0,
            status: None,
            description: None,
        }
    }

    fn message_with_reply(subject: &str, reply: &str, payload: Vec<u8>) -> Message {
        Message {
            subject: subject.to_string().into(),
            reply: Some(reply.to_string().into()),
            payload: payload.into(),
            headers: None,
            length: 0,
            status: None,
            description: None,
        }
    }

    fn list_tools_request(id: i64) -> ClientJsonRpcMessage {
        let request = ClientRequest::ListToolsRequest(ListToolsRequest {
            method: Default::default(),
            params: Some(PaginatedRequestParams::default()),
            extensions: Default::default(),
        });
        ClientJsonRpcMessage::request(request, RequestId::Number(id))
    }

    #[tokio::test]
    async fn client_transport_sends_requests_to_server_method_subject() {
        let nats = AdvancedMockNatsClient::new();
        let _inbound = nats.inject_messages();
        let response = ServerJsonRpcMessage::response(ServerResult::empty(()), RequestId::Number(7));
        nats.set_response(
            "mcp.server.filesystem.tools.list",
            serde_json::to_vec(&response).unwrap().into(),
        );
        let mut transport: NatsTransport<RoleClient, AdvancedMockNatsClient> = NatsTransport::for_client(
            nats.clone(),
            &config(),
            McpPeerId::new("desktop").unwrap(),
            McpPeerId::new("filesystem").unwrap(),
        )
        .await
        .unwrap();

        transport.send(list_tools_request(7)).await.unwrap();

        assert!(matches!(
            transport.receive().await.unwrap(),
            ServerJsonRpcMessage::Response(_)
        ));
    }

    #[tokio::test]
    async fn client_transport_publishes_notifications_to_server_method_subject() {
        let nats = AdvancedMockNatsClient::new();
        let _inbound = nats.inject_messages();
        let mut transport: NatsTransport<RoleClient, AdvancedMockNatsClient> = NatsTransport::for_client(
            nats.clone(),
            &config(),
            McpPeerId::new("desktop").unwrap(),
            McpPeerId::new("filesystem").unwrap(),
        )
        .await
        .unwrap();

        let notification = ClientJsonRpcMessage::notification(
            rmcp::model::ClientNotification::InitializedNotification(Default::default()),
        );
        transport.send(notification).await.unwrap();

        assert_eq!(
            nats.published_messages(),
            vec!["mcp.server.filesystem.notifications.initialized"]
        );
    }

    #[tokio::test]
    async fn server_transport_receives_client_request_from_subscription() {
        let nats = AdvancedMockNatsClient::new();
        let inbound = nats.inject_messages();
        let mut transport: NatsTransport<RoleServer, AdvancedMockNatsClient> = NatsTransport::for_server(
            nats,
            &config(),
            McpPeerId::new("filesystem").unwrap(),
            McpPeerId::new("desktop").unwrap(),
        )
        .await
        .unwrap();
        let request =
            ClientJsonRpcMessage::request(ClientRequest::PingRequest(PingRequest::default()), RequestId::Number(9));

        inbound
            .unbounded_send(message(
                "mcp.server.filesystem.ping",
                serde_json::to_vec(&request).unwrap(),
            ))
            .unwrap();

        assert!(matches!(
            transport.receive().await.unwrap(),
            ClientJsonRpcMessage::Request(_)
        ));
    }

    #[tokio::test]
    async fn server_transport_publishes_response_to_remembered_reply_subject() {
        let nats = AdvancedMockNatsClient::new();
        let inbound = nats.inject_messages();
        let mut transport: NatsTransport<RoleServer, AdvancedMockNatsClient> = NatsTransport::for_server(
            nats.clone(),
            &config(),
            McpPeerId::new("filesystem").unwrap(),
            McpPeerId::new("desktop").unwrap(),
        )
        .await
        .unwrap();
        let request =
            ClientJsonRpcMessage::request(ClientRequest::PingRequest(PingRequest::default()), RequestId::Number(9));

        inbound
            .unbounded_send(message_with_reply(
                "mcp.server.filesystem.ping",
                "_INBOX.desktop.1",
                serde_json::to_vec(&request).unwrap(),
            ))
            .unwrap();

        assert!(matches!(
            transport.receive().await.unwrap(),
            ClientJsonRpcMessage::Request(_)
        ));
        transport
            .send(ServerJsonRpcMessage::response(
                ServerResult::empty(()),
                RequestId::Number(9),
            ))
            .await
            .unwrap();

        assert_eq!(nats.published_messages(), vec!["_INBOX.desktop.1"]);
    }

    #[tokio::test]
    async fn server_transport_keeps_reply_subject_until_response_publish_succeeds() {
        let nats = AdvancedMockNatsClient::new();
        let inbound = nats.inject_messages();
        let mut transport: NatsTransport<RoleServer, AdvancedMockNatsClient> = NatsTransport::for_server(
            nats.clone(),
            &config(),
            McpPeerId::new("filesystem").unwrap(),
            McpPeerId::new("desktop").unwrap(),
        )
        .await
        .unwrap();
        let request = ClientJsonRpcMessage::request(
            ClientRequest::PingRequest(PingRequest::default()),
            RequestId::Number(19),
        );

        inbound
            .unbounded_send(message_with_reply(
                "mcp.server.filesystem.ping",
                "_INBOX.desktop.retry",
                serde_json::to_vec(&request).unwrap(),
            ))
            .unwrap();

        assert!(matches!(
            transport.receive().await.unwrap(),
            ClientJsonRpcMessage::Request(_)
        ));
        let response = ServerJsonRpcMessage::response(ServerResult::empty(()), RequestId::Number(19));
        nats.fail_next_publish();

        let failed = transport.send(response.clone()).await;
        assert!(matches!(failed, Err(NatsTransportError::Publish { .. })));

        transport.send(response.clone()).await.unwrap();
        assert_eq!(nats.published_messages(), vec!["_INBOX.desktop.retry"]);

        let duplicate = transport.send(response).await;
        assert!(matches!(duplicate, Err(NatsTransportError::MissingReplySubject)));
    }

    #[tokio::test]
    async fn server_transport_publishes_error_to_remembered_reply_subject() {
        let nats = AdvancedMockNatsClient::new();
        let inbound = nats.inject_messages();
        let mut transport: NatsTransport<RoleServer, AdvancedMockNatsClient> = NatsTransport::for_server(
            nats.clone(),
            &config(),
            McpPeerId::new("filesystem").unwrap(),
            McpPeerId::new("desktop").unwrap(),
        )
        .await
        .unwrap();
        let request =
            ClientJsonRpcMessage::request(ClientRequest::PingRequest(PingRequest::default()), RequestId::Number(9));

        inbound
            .unbounded_send(message_with_reply(
                "mcp.server.filesystem.ping",
                "_INBOX.desktop.2",
                serde_json::to_vec(&request).unwrap(),
            ))
            .unwrap();

        assert!(matches!(
            transport.receive().await.unwrap(),
            ClientJsonRpcMessage::Request(_)
        ));
        transport
            .send(ServerJsonRpcMessage::error(
                ErrorData::internal_error("request failed", None),
                RequestId::Number(9),
            ))
            .await
            .unwrap();

        assert_eq!(nats.published_messages(), vec!["_INBOX.desktop.2"]);
    }

    #[tokio::test]
    async fn server_transport_publishes_notifications_to_client_method_subject() {
        let nats = AdvancedMockNatsClient::new();
        let _inbound = nats.inject_messages();
        let mut transport: NatsTransport<RoleServer, AdvancedMockNatsClient> = NatsTransport::for_server(
            nats.clone(),
            &config(),
            McpPeerId::new("filesystem").unwrap(),
            McpPeerId::new("desktop").unwrap(),
        )
        .await
        .unwrap();

        let notification =
            ServerJsonRpcMessage::notification(ServerNotification::ToolListChangedNotification(Default::default()));
        transport.send(notification).await.unwrap();

        assert_eq!(
            nats.published_messages(),
            vec!["mcp.client.desktop.notifications.tools.list_changed"]
        );
    }

    #[tokio::test]
    async fn server_transport_rejects_response_without_reply_subject() {
        let nats = AdvancedMockNatsClient::new();
        let _inbound = nats.inject_messages();
        let mut transport: NatsTransport<RoleServer, AdvancedMockNatsClient> = NatsTransport::for_server(
            nats,
            &config(),
            McpPeerId::new("filesystem").unwrap(),
            McpPeerId::new("desktop").unwrap(),
        )
        .await
        .unwrap();

        let result = transport
            .send(ServerJsonRpcMessage::response(
                ServerResult::empty(()),
                RequestId::Number(9),
            ))
            .await;

        assert!(matches!(result, Err(NatsTransportError::MissingReplySubject)));
    }

    #[tokio::test]
    async fn server_transport_receives_client_notification_without_reply_subject() {
        let nats = AdvancedMockNatsClient::new();
        let inbound = nats.inject_messages();
        let mut transport: NatsTransport<RoleServer, AdvancedMockNatsClient> = NatsTransport::for_server(
            nats,
            &config(),
            McpPeerId::new("filesystem").unwrap(),
            McpPeerId::new("desktop").unwrap(),
        )
        .await
        .unwrap();
        let notification = ClientJsonRpcMessage::notification(
            rmcp::model::ClientNotification::InitializedNotification(Default::default()),
        );

        inbound
            .unbounded_send(message(
                "mcp.server.filesystem.notifications.initialized",
                serde_json::to_vec(&notification).unwrap(),
            ))
            .unwrap();

        assert!(matches!(
            transport.receive().await.unwrap(),
            ClientJsonRpcMessage::Notification(_)
        ));
    }

    #[tokio::test]
    async fn transport_close_is_a_noop() {
        let nats = AdvancedMockNatsClient::new();
        let _inbound = nats.inject_messages();
        let mut transport: NatsTransport<RoleClient, AdvancedMockNatsClient> = NatsTransport::for_client(
            nats,
            &config(),
            McpPeerId::new("desktop").unwrap(),
            McpPeerId::new("filesystem").unwrap(),
        )
        .await
        .unwrap();

        transport.close().await.unwrap();
    }

    #[tokio::test]
    async fn transport_skips_invalid_subscription_payloads() {
        let nats = AdvancedMockNatsClient::new();
        let inbound = nats.inject_messages();
        let mut transport: NatsTransport<RoleServer, AdvancedMockNatsClient> = NatsTransport::for_server(
            nats,
            &config(),
            McpPeerId::new("filesystem").unwrap(),
            McpPeerId::new("desktop").unwrap(),
        )
        .await
        .unwrap();
        let request = ClientJsonRpcMessage::request(
            ClientRequest::PingRequest(PingRequest::default()),
            RequestId::Number(10),
        );

        inbound
            .unbounded_send(message("mcp.server.filesystem.ping", b"not-json".to_vec()))
            .unwrap();
        inbound
            .unbounded_send(message(
                "mcp.server.filesystem.ping",
                serde_json::to_vec(&request).unwrap(),
            ))
            .unwrap();

        assert!(matches!(
            transport.receive().await.unwrap(),
            ClientJsonRpcMessage::Request(_)
        ));
    }

    #[tokio::test]
    async fn client_transport_subscribe_failure_is_reported() {
        let nats = AdvancedMockNatsClient::new();

        let result = NatsTransport::<RoleClient, AdvancedMockNatsClient>::for_client(
            nats,
            &config(),
            McpPeerId::new("desktop").unwrap(),
            McpPeerId::new("filesystem").unwrap(),
        )
        .await;

        assert!(matches!(result, Err(NatsTransportError::Subscribe { .. })));
    }

    #[tokio::test]
    async fn client_transport_request_failure_is_reported() {
        let nats = AdvancedMockNatsClient::new();
        let _inbound = nats.inject_messages();
        nats.fail_next_request();
        let mut transport: NatsTransport<RoleClient, AdvancedMockNatsClient> = NatsTransport::for_client(
            nats,
            &config(),
            McpPeerId::new("desktop").unwrap(),
            McpPeerId::new("filesystem").unwrap(),
        )
        .await
        .unwrap();

        let result = transport.send(list_tools_request(11)).await;

        assert!(matches!(result, Err(NatsTransportError::Request { .. })));
    }

    #[tokio::test(start_paused = true)]
    async fn client_transport_request_timeout_is_reported() {
        let nats = AdvancedMockNatsClient::new();
        let _inbound = nats.inject_messages();
        nats.hang_next_request();
        let mut transport: NatsTransport<RoleClient, AdvancedMockNatsClient> = NatsTransport::for_client(
            nats,
            &timeout_config(),
            McpPeerId::new("desktop").unwrap(),
            McpPeerId::new("filesystem").unwrap(),
        )
        .await
        .unwrap();

        let result = transport.send(list_tools_request(12)).await;

        assert!(matches!(result, Err(NatsTransportError::RequestTimedOut { .. })));
    }

    #[tokio::test(start_paused = true)]
    async fn client_transport_publish_timeout_is_reported() {
        let nats = AdvancedMockNatsClient::new();
        let _inbound = nats.inject_messages();
        nats.hang_next_publish();
        let mut transport: NatsTransport<RoleClient, AdvancedMockNatsClient> = NatsTransport::for_client(
            nats,
            &timeout_config(),
            McpPeerId::new("desktop").unwrap(),
            McpPeerId::new("filesystem").unwrap(),
        )
        .await
        .unwrap();
        let notification = ClientJsonRpcMessage::notification(
            rmcp::model::ClientNotification::InitializedNotification(Default::default()),
        );

        let result = transport.send(notification).await;

        assert!(matches!(result, Err(NatsTransportError::PublishTimedOut { .. })));
    }

    #[tokio::test]
    async fn client_transport_publish_failure_is_reported() {
        let nats = AdvancedMockNatsClient::new();
        let _inbound = nats.inject_messages();
        nats.fail_next_publish();
        let mut transport: NatsTransport<RoleClient, AdvancedMockNatsClient> = NatsTransport::for_client(
            nats,
            &config(),
            McpPeerId::new("desktop").unwrap(),
            McpPeerId::new("filesystem").unwrap(),
        )
        .await
        .unwrap();
        let notification = ClientJsonRpcMessage::notification(
            rmcp::model::ClientNotification::InitializedNotification(Default::default()),
        );

        let result = transport.send(notification).await;

        assert!(matches!(result, Err(NatsTransportError::Publish { .. })));
    }

    #[tokio::test]
    async fn client_transport_flush_failure_is_reported() {
        let nats = AdvancedMockNatsClient::new();
        let _inbound = nats.inject_messages();
        nats.fail_next_flush();
        let mut transport: NatsTransport<RoleClient, AdvancedMockNatsClient> = NatsTransport::for_client(
            nats,
            &config(),
            McpPeerId::new("desktop").unwrap(),
            McpPeerId::new("filesystem").unwrap(),
        )
        .await
        .unwrap();
        let notification = ClientJsonRpcMessage::notification(
            rmcp::model::ClientNotification::InitializedNotification(Default::default()),
        );

        let result = transport.send(notification).await;

        assert!(matches!(result, Err(NatsTransportError::Flush { .. })));
    }

    #[tokio::test]
    async fn method_suffix_rejects_unknown_methods() {
        assert!(matches!(
            method_suffix("custom/unknown"),
            Err(NatsTransportError::UnsupportedMethod { .. })
        ));
    }

    #[test]
    fn method_suffix_maps_rmcp_methods_to_acp_style_subject_suffixes() {
        assert_eq!(method_suffix("tools/list").unwrap(), "tools.list");
        assert_eq!(
            method_suffix("sampling/createMessage").unwrap(),
            "sampling.create_message"
        );
        assert_eq!(
            method_suffix("notifications/tools/list_changed").unwrap(),
            "notifications.tools.list_changed"
        );
    }

    #[test]
    fn transport_error_display_and_source_are_specific() {
        let json_error = || serde_json::from_str::<serde_json::Value>("").unwrap_err();

        let subscribe = NatsTransportError::Subscribe {
            source: Box::new(MockError("nats".to_string())),
        };
        assert_eq!(subscribe.to_string(), "failed to subscribe to MCP NATS subject");
        assert!(std::error::Error::source(&subscribe).is_some());

        let request = NatsTransportError::Request {
            subject: "mcp.server.filesystem.ping".to_string(),
            source: Box::new(MockError("nats".to_string())),
        };
        assert_eq!(
            request.to_string(),
            "failed to request MCP NATS subject mcp.server.filesystem.ping"
        );
        assert!(std::error::Error::source(&request).is_some());

        let timeout = NatsTransportError::RequestTimedOut {
            subject: "mcp.server.filesystem.ping".to_string(),
        };
        assert_eq!(
            timeout.to_string(),
            "timed out requesting MCP NATS subject mcp.server.filesystem.ping"
        );
        assert!(std::error::Error::source(&timeout).is_none());

        let publish = NatsTransportError::Publish {
            subject: "mcp.server.filesystem.notifications.initialized".to_string(),
            source: Box::new(MockError("nats".to_string())),
        };
        assert_eq!(
            publish.to_string(),
            "failed to publish MCP NATS subject mcp.server.filesystem.notifications.initialized"
        );
        assert!(std::error::Error::source(&publish).is_some());

        let publish_timeout = NatsTransportError::PublishTimedOut {
            subject: "mcp.server.filesystem.notifications.initialized".to_string(),
        };
        assert_eq!(
            publish_timeout.to_string(),
            "timed out publishing MCP NATS subject mcp.server.filesystem.notifications.initialized"
        );
        assert!(std::error::Error::source(&publish_timeout).is_none());

        let flush = NatsTransportError::Flush {
            source: Box::new(MockError("nats".to_string())),
        };
        assert_eq!(flush.to_string(), "failed to flush MCP NATS client");
        assert!(std::error::Error::source(&flush).is_some());

        let serialize = NatsTransportError::Serialize(json_error());
        assert_eq!(serialize.to_string(), "failed to serialize MCP JSON-RPC message");
        assert!(std::error::Error::source(&serialize).is_some());

        let deserialize = NatsTransportError::Deserialize(json_error());
        assert_eq!(deserialize.to_string(), "failed to deserialize MCP JSON-RPC message");
        assert!(std::error::Error::source(&deserialize).is_some());

        let missing_method = NatsTransportError::MissingMethod;
        assert_eq!(missing_method.to_string(), "MCP JSON-RPC message is missing a method");
        assert!(std::error::Error::source(&missing_method).is_none());

        let unsupported = NatsTransportError::UnsupportedMethod {
            method: "custom/unknown".to_string(),
        };
        assert_eq!(
            unsupported.to_string(),
            "unsupported MCP method for NATS routing: custom/unknown"
        );
        assert!(std::error::Error::source(&unsupported).is_none());

        let missing_reply = NatsTransportError::MissingReplySubject;
        assert_eq!(missing_reply.to_string(), "missing reply subject for MCP response");
        assert!(std::error::Error::source(&missing_reply).is_none());

        let inbound_closed = NatsTransportError::InboundClosed;
        assert_eq!(inbound_closed.to_string(), "MCP NATS inbound queue is closed");
        assert!(std::error::Error::source(&inbound_closed).is_none());
    }
}
