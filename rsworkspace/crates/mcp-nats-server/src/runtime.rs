use std::collections::HashMap;
use std::future;
use std::io;
use std::time::Duration;

use mcp_nats::{
    ClientJsonRpcMessage, Config, ErrorData, FlushClient, McpPeerId, NatsTransport, PublishClient, RequestClient,
    RequestId, ServerJsonRpcMessage, SubscribeClient,
};
use rmcp::model::{ClientNotification, ClientRequest, ServerInfo, ServerRequest, ServerResult};
use rmcp::service::{NotificationContext, Peer, RequestContext, RoleClient, RoleServer, Service, ServiceError};
use rmcp::transport::Transport;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tracing::warn;
use uuid::Uuid;

use crate::allowed_host::AllowedHost;

type ProxyResponse = oneshot::Sender<Result<ServerResult, ErrorData>>;
type ProxyAck = oneshot::Sender<Result<(), ErrorData>>;

struct PendingEntry {
    response_tx: ProxyResponse,
    deadline: Instant,
}

pub fn streamable_http_config(allowed_hosts: Vec<AllowedHost>) -> StreamableHttpServerConfig {
    let config = StreamableHttpServerConfig::default();
    if allowed_hosts.is_empty() {
        config
    } else {
        config.with_allowed_hosts(
            allowed_hosts
                .iter()
                .map(|allowed_host| allowed_host.as_str().to_string())
                .collect::<Vec<_>>(),
        )
    }
}

#[derive(Clone)]
pub struct ClientIdFactory {
    prefix: McpPeerId,
}

impl ClientIdFactory {
    pub fn new(prefix: McpPeerId) -> Self {
        Self { prefix }
    }

    pub fn next(&self) -> Result<McpPeerId, mcp_nats::McpPeerIdError> {
        McpPeerId::new(format!("{}-{}", self.prefix.as_str(), Uuid::now_v7().simple()))
    }
}

pub fn streamable_http_service<N>(
    nats: N,
    config: Config,
    client_ids: ClientIdFactory,
    server_id: McpPeerId,
    http_config: StreamableHttpServerConfig,
) -> StreamableHttpService<McpNatsProxyService<N>, LocalSessionManager>
where
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    N::SubscribeError: 'static,
    N::RequestError: 'static,
    N::PublishError: 'static,
    N::FlushError: 'static,
{
    StreamableHttpService::new(
        move || {
            let client_id = client_ids
                .next()
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
            Ok(McpNatsProxyService::new(
                nats.clone(),
                config.clone(),
                client_id,
                server_id.clone(),
            ))
        },
        Default::default(),
        http_config,
    )
}

pub struct McpNatsProxyService<N>
where
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
{
    command_tx: mpsc::Sender<ProxyCommand>,
    server_info: ServerInfo,
    _nats: std::marker::PhantomData<N>,
}

impl<N> McpNatsProxyService<N>
where
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    N::SubscribeError: 'static,
    N::RequestError: 'static,
    N::PublishError: 'static,
    N::FlushError: 'static,
{
    pub fn new(nats: N, config: Config, client_id: McpPeerId, server_id: McpPeerId) -> Self {
        let (command_tx, command_rx) = mpsc::channel(64);
        tokio::spawn(run_proxy_worker(nats, config, client_id, server_id, command_rx));
        Self {
            command_tx,
            server_info: ServerInfo::default(),
            _nats: std::marker::PhantomData,
        }
    }
}

impl<N> Service<RoleServer> for McpNatsProxyService<N>
where
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
{
    async fn handle_request(
        &self,
        request: ClientRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<ServerResult, ErrorData> {
        let (response_tx, response_rx) = oneshot::channel();
        self.command_tx
            .send(ProxyCommand::Request {
                request: Box::new(request),
                request_id: context.id,
                peer: context.peer,
                response_tx,
            })
            .await
            .map_err(|_| ErrorData::internal_error("MCP NATS proxy is unavailable", None))?;
        response_rx
            .await
            .map_err(|_| ErrorData::internal_error("MCP NATS proxy dropped the request", None))?
    }

    async fn handle_notification(
        &self,
        notification: ClientNotification,
        context: NotificationContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        let (response_tx, response_rx) = oneshot::channel();
        self.command_tx
            .send(ProxyCommand::Notification {
                notification,
                peer: context.peer,
                response_tx,
            })
            .await
            .map_err(|_| ErrorData::internal_error("MCP NATS proxy is unavailable", None))?;
        response_rx
            .await
            .map_err(|_| ErrorData::internal_error("MCP NATS proxy dropped the notification", None))?
    }

    fn get_info(&self) -> ServerInfo {
        self.server_info.clone()
    }
}

enum ProxyCommand {
    Request {
        request: Box<ClientRequest>,
        request_id: RequestId,
        peer: Peer<RoleServer>,
        response_tx: ProxyResponse,
    },
    Notification {
        notification: ClientNotification,
        peer: Peer<RoleServer>,
        response_tx: ProxyAck,
    },
}

async fn run_proxy_worker<N>(
    nats: N,
    config: Config,
    client_id: McpPeerId,
    server_id: McpPeerId,
    mut command_rx: mpsc::Receiver<ProxyCommand>,
) where
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    N::SubscribeError: 'static,
    N::RequestError: 'static,
    N::PublishError: 'static,
    N::FlushError: 'static,
{
    let mut transport = match mcp_nats::client::connect(nats, &config, client_id.clone(), server_id).await {
        Ok(transport) => transport,
        Err(error) => {
            fail_commands(command_rx, ErrorData::internal_error(error.to_string(), None)).await;
            return;
        }
    };
    let operation_timeout = config.operation_timeout();
    let mut peer = None;
    let mut pending: HashMap<RequestId, PendingEntry> = HashMap::new();

    loop {
        let next_deadline = pending.values().map(|entry| entry.deadline).min();
        tokio::select! {
            command = command_rx.recv() => {
                let Some(command) = command else {
                    break;
                };
                handle_proxy_command(command, &mut transport, &mut peer, &mut pending, operation_timeout).await;
            }
            message = transport.receive() => {
                let Some(message) = message else {
                    fail_pending(pending, ErrorData::internal_error("MCP NATS transport closed", None));
                    break;
                };
                handle_remote_message(message, &mut transport, peer.as_ref(), &mut pending).await;
            }
            () = wait_for_deadline(next_deadline) => {
                evict_expired_pending(&mut pending);
            }
        }
    }

    if let Err(error) = transport.close().await {
        warn!(error = %error, "Failed to close MCP NATS proxy transport");
    }
}

async fn handle_proxy_command<N>(
    command: ProxyCommand,
    transport: &mut NatsTransport<RoleClient, N>,
    peer: &mut Option<Peer<RoleServer>>,
    pending: &mut HashMap<RequestId, PendingEntry>,
    operation_timeout: Duration,
) where
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    N::RequestError: 'static,
    N::PublishError: 'static,
    N::FlushError: 'static,
{
    match command {
        ProxyCommand::Request {
            request,
            request_id,
            peer: request_peer,
            response_tx,
        } => {
            *peer = Some(request_peer);
            let message = ClientJsonRpcMessage::request(*request, request_id.clone());
            pending.insert(
                request_id.clone(),
                PendingEntry {
                    response_tx,
                    deadline: Instant::now() + operation_timeout,
                },
            );
            if let Err(error) = transport.send(message).await
                && let Some(entry) = pending.remove(&request_id)
            {
                let _ = entry
                    .response_tx
                    .send(Err(ErrorData::internal_error(error.to_string(), None)));
            }
        }
        ProxyCommand::Notification {
            notification,
            peer: notification_peer,
            response_tx,
        } => {
            *peer = Some(notification_peer);
            let result = transport
                .send(ClientJsonRpcMessage::notification(notification))
                .await
                .map_err(|error| ErrorData::internal_error(error.to_string(), None));
            let _ = response_tx.send(result);
        }
    }
}

async fn handle_remote_message<N>(
    message: ServerJsonRpcMessage,
    transport: &mut NatsTransport<RoleClient, N>,
    peer: Option<&Peer<RoleServer>>,
    pending: &mut HashMap<RequestId, PendingEntry>,
) where
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    N::RequestError: 'static,
    N::PublishError: 'static,
    N::FlushError: 'static,
{
    match message {
        ServerJsonRpcMessage::Response(response) => {
            if let Some(entry) = pending.remove(&response.id) {
                let _ = entry.response_tx.send(Ok(response.result));
            }
        }
        ServerJsonRpcMessage::Error(error) => {
            if let Some(entry) = pending.remove(&error.id) {
                let _ = entry.response_tx.send(Err(error.error));
            }
        }
        ServerJsonRpcMessage::Notification(notification) => {
            if let Some(peer) = peer
                && let Err(error) = peer.send_notification(notification.notification).await
            {
                warn!(error = %error, "Failed to forward MCP server notification to HTTP client");
            }
        }
        ServerJsonRpcMessage::Request(request) => {
            forward_server_request_to_http_client(request.request, request.id, peer, transport).await;
        }
    }
}

async fn forward_server_request_to_http_client<N>(
    request: ServerRequest,
    request_id: RequestId,
    peer: Option<&Peer<RoleServer>>,
    transport: &mut NatsTransport<RoleClient, N>,
) where
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    N::RequestError: 'static,
    N::PublishError: 'static,
    N::FlushError: 'static,
{
    let message = match peer {
        Some(peer) => match peer.send_request(request).await {
            Ok(result) => ClientJsonRpcMessage::response(result, request_id),
            Err(error) => ClientJsonRpcMessage::error(service_error_to_error_data(error), request_id),
        },
        None => ClientJsonRpcMessage::error(
            ErrorData::internal_error("MCP HTTP client is not available", None),
            request_id,
        ),
    };

    if let Err(error) = transport.send(message).await {
        warn!(error = %error, "Failed to forward MCP HTTP client response to NATS");
    }
}

fn service_error_to_error_data(error: ServiceError) -> ErrorData {
    ErrorData::internal_error(error.to_string(), None)
}

fn fail_pending(pending: HashMap<RequestId, PendingEntry>, error: ErrorData) {
    for entry in pending.into_values() {
        let _ = entry.response_tx.send(Err(error.clone()));
    }
}

async fn wait_for_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => future::pending().await,
    }
}

fn evict_expired_pending(pending: &mut HashMap<RequestId, PendingEntry>) {
    let now = Instant::now();
    let expired = pending
        .iter()
        .filter(|(_, entry)| entry.deadline <= now)
        .map(|(request_id, _)| request_id.clone())
        .collect::<Vec<_>>();
    for request_id in expired {
        if let Some(entry) = pending.remove(&request_id) {
            let _ = entry.response_tx.send(Err(ErrorData::internal_error(
                "MCP NATS proxy timed out waiting for a response",
                None,
            )));
        }
    }
}

async fn fail_commands(mut command_rx: mpsc::Receiver<ProxyCommand>, error: ErrorData) {
    while let Some(command) = command_rx.recv().await {
        match command {
            ProxyCommand::Request { response_tx, .. } => {
                let _ = response_tx.send(Err(error.clone()));
            }
            ProxyCommand::Notification { response_tx, .. } => {
                let _ = response_tx.send(Err(error.clone()));
            }
        }
    }
}

#[cfg(test)]
mod tests;
