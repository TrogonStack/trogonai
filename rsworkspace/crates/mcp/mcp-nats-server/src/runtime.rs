use std::collections::HashMap;
use std::future;
use std::io;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::http::request::Parts;
use mcp_nats::{
    ClientJsonRpcMessage, Config, ErrorData, FlushClient, McpPeerId, McpTransportHeaders, NatsTransport, PublishClient,
    RequestClient, RequestId, ServerJsonRpcMessage, SubscribeClient,
};
use rmcp::ServerHandler;
#[allow(deprecated)]
use rmcp::model::SetLevelRequestParams;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CancelTaskParams, CancelledNotificationParam, ClientNotification,
    ClientRequest, CompleteRequestParams, CompleteResult, CustomNotification, CustomRequest, CustomResult,
    DiscoverRequestParams, DiscoverResult, Extensions, GetPromptRequestParams, GetPromptResponse, GetTaskParams,
    GetTaskResult, InitializeRequestParams, InitializeResult, ListPromptsResult, ListResourceTemplatesResult,
    ListResourcesResult, ListToolsResult, Notification, NotificationMetaObject, NotificationNoParam,
    PaginatedRequestParams, ProgressNotificationParam, ReadResourceRequestParams, ReadResourceResponse, Request,
    RequestMetaObject, RequestNoParam, RequestOptionalParam, ServerInfo, ServerRequest, ServerResult,
    SubscribeRequestParams, UnsubscribeRequestParams, UpdateTaskParams,
};
use rmcp::service::{NotificationContext, Peer, RequestContext, RoleClient, RoleServer, ServiceError};
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
    operation_timeout: Duration,
    server_info: Arc<RwLock<ServerInfo>>,
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
        let operation_timeout = config.operation_timeout();
        tokio::spawn(run_proxy_worker(nats, config, client_id, server_id, command_rx));
        Self {
            command_tx,
            operation_timeout,
            server_info: Arc::new(RwLock::new(ServerInfo::default())),
            _nats: std::marker::PhantomData,
        }
    }
}

impl<N> McpNatsProxyService<N>
where
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
{
    fn remember_server_info(&self, result: &ServerInfo) {
        let mut server_info = self
            .server_info
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *server_info = result.clone();
    }

    async fn forward(
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
        tokio::time::timeout(self.operation_timeout, response_rx)
            .await
            .map_err(|_| ErrorData::internal_error("MCP NATS proxy timed out waiting for a response", None))?
            .map_err(|_| ErrorData::internal_error("MCP NATS proxy dropped the request", None))?
    }

    async fn forward_notification(
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
        tokio::time::timeout(self.operation_timeout, response_rx)
            .await
            .map_err(|_| ErrorData::internal_error("MCP NATS proxy timed out waiting for the notification", None))?
            .map_err(|_| ErrorData::internal_error("MCP NATS proxy dropped the notification", None))?
    }
}

fn unexpected_result(method: &str, result: &ServerResult) -> ErrorData {
    // Keep the payload-bearing debug local; the client-facing message names only
    // the method so tool/prompt content cannot leak through the error.
    warn!(method, ?result, "MCP NATS proxy received an unexpected result");
    ErrorData::internal_error(
        format!("MCP NATS proxy received an unexpected result for {method}"),
        None,
    )
}

fn preserve_http_transport_headers(extensions: &mut Extensions) {
    let headers = extensions
        .get::<Parts>()
        .map(|parts| McpTransportHeaders::from_http(&parts.headers));
    if let Some(headers) = headers.filter(|headers| !headers.is_empty()) {
        extensions.insert(headers);
    }
}

fn restore_request_meta(extensions: &mut Extensions, context_meta: RequestMetaObject) {
    let mut meta = extensions.remove::<RequestMetaObject>().unwrap_or_default();
    meta.extend(context_meta);
    if !meta.is_empty() {
        extensions.insert(meta);
    }
}

fn restore_notification_meta(extensions: &mut Extensions, context_meta: NotificationMetaObject) {
    let mut meta = extensions.remove::<NotificationMetaObject>().unwrap_or_default();
    meta.extend(context_meta);
    if !meta.is_empty() {
        extensions.insert(meta);
    }
}

fn request_extensions(context: &RequestContext<RoleServer>) -> Extensions {
    let mut extensions = context.extensions.clone();
    restore_request_meta(&mut extensions, context.meta.clone());
    preserve_http_transport_headers(&mut extensions);
    extensions
}

fn notification_extensions(context: &NotificationContext<RoleServer>) -> Extensions {
    let mut extensions = context.extensions.clone();
    restore_notification_meta(&mut extensions, context.meta.clone());
    preserve_http_transport_headers(&mut extensions);
    extensions
}

fn wrap_request<M: Default, P>(params: P, context: &RequestContext<RoleServer>) -> Request<M, P> {
    let mut request = Request::new(params);
    request.extensions = request_extensions(context);
    request
}

fn wrap_optional_request<M: Default, P>(
    params: Option<P>,
    context: &RequestContext<RoleServer>,
) -> RequestOptionalParam<M, P> {
    RequestOptionalParam {
        method: Default::default(),
        params,
        extensions: request_extensions(context),
    }
}

fn wrap_no_param_request<M: Default>(context: &RequestContext<RoleServer>) -> RequestNoParam<M> {
    RequestNoParam {
        method: Default::default(),
        extensions: request_extensions(context),
    }
}

fn wrap_notification<M: Default, P>(params: P, context: &NotificationContext<RoleServer>) -> Notification<M, P> {
    let mut notification = Notification::new(params);
    notification.extensions = notification_extensions(context);
    notification
}

fn wrap_no_param_notification<M: Default>(context: &NotificationContext<RoleServer>) -> NotificationNoParam<M> {
    NotificationNoParam {
        method: Default::default(),
        extensions: notification_extensions(context),
    }
}

impl<N> ServerHandler for McpNatsProxyService<N>
where
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
{
    async fn ping(&self, context: RequestContext<RoleServer>) -> Result<(), ErrorData> {
        let request = ClientRequest::PingRequest(wrap_no_param_request(&context));
        match self.forward(request, context).await? {
            ServerResult::EmptyResult(_) => Ok(()),
            other => Err(unexpected_result("ping", &other)),
        }
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        context.peer.set_peer_info(request.clone());
        let wire_request = ClientRequest::InitializeRequest(wrap_request(request, &context));
        match self.forward(wire_request, context).await? {
            ServerResult::InitializeResult(result) => {
                self.remember_server_info(&result);
                Ok(result)
            }
            other => Err(unexpected_result("initialize", &other)),
        }
    }

    async fn discover(&self, context: RequestContext<RoleServer>) -> Result<DiscoverResult, ErrorData> {
        let request = ClientRequest::DiscoverRequest(wrap_request(DiscoverRequestParams {}, &context));
        match self.forward(request, context).await? {
            ServerResult::DiscoverResult(result) => Ok(result),
            other => Err(unexpected_result("discover", &other)),
        }
    }

    async fn complete(
        &self,
        request: CompleteRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, ErrorData> {
        let wire_request = ClientRequest::CompleteRequest(wrap_request(request, &context));
        match self.forward(wire_request, context).await? {
            ServerResult::CompleteResult(result) => Ok(result),
            other => Err(unexpected_result("completion/complete", &other)),
        }
    }

    #[allow(deprecated)]
    async fn set_level(
        &self,
        request: SetLevelRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        let wire_request = ClientRequest::SetLevelRequest(wrap_request(request, &context));
        match self.forward(wire_request, context).await? {
            ServerResult::EmptyResult(_) => Ok(()),
            other => Err(unexpected_result("logging/setLevel", &other)),
        }
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        let wire_request = ClientRequest::GetPromptRequest(wrap_request(request, &context));
        match self.forward(wire_request, context).await? {
            ServerResult::GetPromptResult(result) => Ok(GetPromptResponse::Complete(result)),
            ServerResult::InputRequiredResult(result) => Ok(GetPromptResponse::InputRequired(result)),
            other => Err(unexpected_result("prompts/get", &other)),
        }
    }

    async fn list_prompts(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        let wire_request = ClientRequest::ListPromptsRequest(wrap_optional_request(request, &context));
        match self.forward(wire_request, context).await? {
            ServerResult::ListPromptsResult(result) => Ok(result),
            other => Err(unexpected_result("prompts/list", &other)),
        }
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let wire_request = ClientRequest::ListResourcesRequest(wrap_optional_request(request, &context));
        match self.forward(wire_request, context).await? {
            ServerResult::ListResourcesResult(result) => Ok(result),
            other => Err(unexpected_result("resources/list", &other)),
        }
    }

    async fn list_resource_templates(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        let wire_request = ClientRequest::ListResourceTemplatesRequest(wrap_optional_request(request, &context));
        match self.forward(wire_request, context).await? {
            ServerResult::ListResourceTemplatesResult(result) => Ok(result),
            other => Err(unexpected_result("resources/templates/list", &other)),
        }
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let wire_request = ClientRequest::ReadResourceRequest(wrap_request(request, &context));
        match self.forward(wire_request, context).await? {
            ServerResult::ReadResourceResult(result) => Ok(ReadResourceResponse::Complete(result)),
            ServerResult::InputRequiredResult(result) => Ok(ReadResourceResponse::InputRequired(result)),
            other => Err(unexpected_result("resources/read", &other)),
        }
    }

    #[allow(deprecated)]
    async fn subscribe(
        &self,
        request: SubscribeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        let wire_request = ClientRequest::SubscribeRequest(wrap_request(request, &context));
        match self.forward(wire_request, context).await? {
            ServerResult::EmptyResult(_) => Ok(()),
            other => Err(unexpected_result("resources/subscribe", &other)),
        }
    }

    #[allow(deprecated)]
    async fn unsubscribe(
        &self,
        request: UnsubscribeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        let wire_request = ClientRequest::UnsubscribeRequest(wrap_request(request, &context));
        match self.forward(wire_request, context).await? {
            ServerResult::EmptyResult(_) => Ok(()),
            other => Err(unexpected_result("resources/unsubscribe", &other)),
        }
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let wire_request = ClientRequest::CallToolRequest(wrap_request(request, &context));
        match self.forward(wire_request, context).await? {
            ServerResult::CallToolResult(result) => Ok(CallToolResponse::Complete(result)),
            ServerResult::InputRequiredResult(result) => Ok(CallToolResponse::InputRequired(result)),
            ServerResult::CreateTaskResult(result) => Ok(CallToolResponse::Task(result)),
            other => Err(unexpected_result("tools/call", &other)),
        }
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let wire_request = ClientRequest::ListToolsRequest(wrap_optional_request(request, &context));
        match self.forward(wire_request, context).await? {
            ServerResult::ListToolsResult(result) => Ok(result),
            other => Err(unexpected_result("tools/list", &other)),
        }
    }

    async fn on_custom_request(
        &self,
        mut request: CustomRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<CustomResult, ErrorData> {
        request.extensions = request_extensions(&context);
        match self.forward(ClientRequest::CustomRequest(request), context).await? {
            ServerResult::CustomResult(result) => Ok(result),
            other => Err(unexpected_result("custom request", &other)),
        }
    }

    async fn get_task(
        &self,
        request: GetTaskParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, ErrorData> {
        let wire_request = ClientRequest::GetTaskRequest(wrap_request(request, &context));
        match self.forward(wire_request, context).await? {
            ServerResult::GetTaskResult(result) => Ok(result),
            other => Err(unexpected_result("tasks/get", &other)),
        }
    }

    async fn update_task(
        &self,
        request: UpdateTaskParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        let wire_request = ClientRequest::UpdateTaskRequest(wrap_request(request, &context));
        match self.forward(wire_request, context).await? {
            ServerResult::TaskAckResult(_) => Ok(()),
            other => Err(unexpected_result("tasks/update", &other)),
        }
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        let wire_request = ClientRequest::CancelTaskRequest(wrap_request(request, &context));
        match self.forward(wire_request, context).await? {
            ServerResult::TaskAckResult(_) => Ok(()),
            other => Err(unexpected_result("tasks/cancel", &other)),
        }
    }

    async fn on_cancelled(&self, notification: CancelledNotificationParam, context: NotificationContext<RoleServer>) {
        let wire_notification = ClientNotification::CancelledNotification(wrap_notification(notification, &context));
        if let Err(error) = self.forward_notification(wire_notification, context).await {
            warn!(error = %error, "Failed to forward cancelled notification to MCP NATS proxy");
        }
    }

    async fn on_progress(&self, notification: ProgressNotificationParam, context: NotificationContext<RoleServer>) {
        let wire_notification = ClientNotification::ProgressNotification(wrap_notification(notification, &context));
        if let Err(error) = self.forward_notification(wire_notification, context).await {
            warn!(error = %error, "Failed to forward progress notification to MCP NATS proxy");
        }
    }

    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        let wire_notification = ClientNotification::InitializedNotification(wrap_no_param_notification(&context));
        if let Err(error) = self.forward_notification(wire_notification, context).await {
            warn!(error = %error, "Failed to forward initialized notification to MCP NATS proxy");
        }
    }

    async fn on_roots_list_changed(&self, context: NotificationContext<RoleServer>) {
        let wire_notification = ClientNotification::RootsListChangedNotification(wrap_no_param_notification(&context));
        if let Err(error) = self.forward_notification(wire_notification, context).await {
            warn!(error = %error, "Failed to forward roots list changed notification to MCP NATS proxy");
        }
    }

    async fn on_custom_notification(
        &self,
        mut notification: CustomNotification,
        context: NotificationContext<RoleServer>,
    ) {
        notification.extensions = notification_extensions(&context);
        let wire_notification = ClientNotification::CustomNotification(notification);
        if let Err(error) = self.forward_notification(wire_notification, context).await {
            warn!(error = %error, "Failed to forward custom notification to MCP NATS proxy");
        }
    }

    fn get_info(&self) -> ServerInfo {
        self.server_info
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
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
            if let Some(id) = &error.id
                && let Some(entry) = pending.remove(id)
            {
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
            Err(error) => ClientJsonRpcMessage::error(service_error_to_error_data(error), Some(request_id)),
        },
        None => ClientJsonRpcMessage::error(
            ErrorData::internal_error("MCP HTTP client is not available", None),
            Some(request_id),
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
