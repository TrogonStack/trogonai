use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::time::Duration;

use async_nats::Message;
use futures::StreamExt;
use jsonrpc_nats::{Direction, TransportError, jsonrpc_publish_with_timeout, jsonrpc_request_raw};
use rmcp::model::{JsonRpcMessage, RequestId};
use rmcp::service::{RoleClient, RoleServer, RxJsonRpcMessage, ServiceRole, TxJsonRpcMessage};
use rmcp::transport::Transport;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tracing::{Instrument, Span, instrument};
use trogon_nats::{FlushClient, PublishClient, RequestClient, SubscribeClient, headers_with_trace_context};

use crate::{Config, McpPeerId, McpPrefix, nats, wire};

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
        let subject = nats::subjects::subscriptions::OneClientSubject::new(config.prefix(), &client_id);
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
        let subject = nats::subjects::subscriptions::OneServerSubject::new(config.prefix(), &server_id);
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
                    let encoded = wire::encode_tx::<R>(&item)?;
                    let headers = wire::merge_headers(headers_with_trace_context(), encoded.headers);
                    let (response_headers, response_body) =
                        jsonrpc_request_raw(&nats, &subject, headers, encoded.body, timeout)
                            .await
                            .map_err(map_transport_error)?;
                    let parsed = wire::decode_rx::<R>(Direction::Response, None, &response_headers, &response_body)?;
                    inbound_tx
                        .send(parsed)
                        .await
                        .map_err(|_| NatsTransportError::InboundClosed)?;
                    Ok(())
                }
                JsonRpcMessage::Notification(_) => {
                    let subject = subject_for_message::<R>(&prefix, &remote_peer_id, &item)?;
                    crate::telemetry::transport::record_subject(&Span::current(), &subject);
                    let encoded = wire::encode_tx::<R>(&item)?;
                    jsonrpc_publish_with_timeout(&nats, &subject, encoded, headers_with_trace_context(), timeout)
                        .await
                        .map_err(map_transport_error)
                }
                JsonRpcMessage::Response(response) => {
                    let request_id = response.id.clone();
                    let subject = reply_subject_for_response(&request_id, &pending_replies)
                        .await
                        .ok_or(NatsTransportError::MissingReplySubject)?;
                    let item: TxJsonRpcMessage<R> = JsonRpcMessage::Response(response);
                    crate::telemetry::transport::record_subject(&Span::current(), &subject);
                    let encoded = wire::encode_tx::<R>(&item)?;
                    jsonrpc_publish_with_timeout(&nats, &subject, encoded, headers_with_trace_context(), timeout)
                        .await
                        .map_err(map_transport_error)?;
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
                    let encoded = wire::encode_tx::<R>(&item)?;
                    jsonrpc_publish_with_timeout(&nats, &subject, encoded, headers_with_trace_context(), timeout)
                        .await
                        .map_err(map_transport_error)?;
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
                    match method_from_subject::<R>(message.subject.as_str()) {
                        Ok(method) => {
                            let headers = message.headers.clone().unwrap_or_default();
                            match wire::decode_rx::<R>(
                                Direction::Request,
                                Some(&method),
                                &headers,
                                &message.payload,
                            ) {
                                Ok(parsed) => {
                                    remember_reply_subject::<R>(&parsed, &message, &self.pending_replies).await;
                                    return Some(parsed);
                                }
                                Err(error) => {
                                    tracing::warn!(error = %error, subject = %message.subject, "Failed to decode MCP NATS message");
                                }
                            }
                        }
                        Err(error) => {
                            tracing::warn!(error = %error, subject = %message.subject, "Failed to resolve MCP method from subject");
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

fn map_transport_error(err: TransportError) -> NatsTransportError {
    match err {
        TransportError::Codec(source) => NatsTransportError::Codec(source),
        TransportError::Timeout { subject } => NatsTransportError::RequestTimedOut { subject },
        TransportError::Request { subject, error } => NatsTransportError::Request {
            subject,
            source: error.into(),
        },
        TransportError::Publish { subject, error } => NatsTransportError::Publish {
            subject,
            source: error.into(),
        },
        TransportError::PublishTimeout { subject } => NatsTransportError::PublishTimedOut { subject },
        TransportError::Flush { error } => NatsTransportError::Flush { source: error.into() },
        TransportError::UnexpectedResponse => NatsTransportError::Deserialize(
            <serde_json::Error as serde::de::Error>::custom("unexpected JSON-RPC response variant"),
        ),
    }
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
mod tests;
