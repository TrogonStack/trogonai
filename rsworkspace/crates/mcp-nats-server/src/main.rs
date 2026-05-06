mod config;

#[cfg(not(coverage))]
mod runtime {
    use std::collections::HashMap;
    use std::error::Error;
    use std::io;

    use axum::Router;
    use mcp_nats::{
        ClientJsonRpcMessage, Config, ErrorData, FlushClient, McpPeerId, NatsTransport, PublishClient, RequestClient,
        RequestId, ServerJsonRpcMessage, SubscribeClient,
    };
    use rmcp::model::{ClientNotification, ClientRequest, ServerInfo, ServerRequest, ServerResult};
    use rmcp::service::{NotificationContext, Peer, RequestContext, RoleClient, RoleServer, Service, ServiceError};
    use rmcp::transport::Transport;
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
    use tokio::net::TcpListener;
    use tokio::sync::{mpsc, oneshot};
    use tracing::{error, info, warn};
    use trogon_std::env::SystemEnv;
    use uuid::Uuid;

    use crate::config;

    type BoxError = Box<dyn Error + Send + Sync>;
    type ProxyResponse = oneshot::Sender<Result<ServerResult, ErrorData>>;
    type ProxyAck = oneshot::Sender<Result<(), ErrorData>>;

    #[tokio::main]
    pub async fn main() -> Result<(), BoxError> {
        init_logging();

        let config = config::base_config(&trogon_std::CliArgs::<config::Args>::new(), &SystemEnv)?;
        let config::HttpBridgeConfig {
            mcp,
            client_id_prefix,
            server_id,
            bind_addr,
            path,
            allowed_hosts,
        } = config;
        let mcp = mcp_nats::apply_timeout_overrides(mcp, &SystemEnv);

        let nats_connect_timeout = mcp_nats::nats_connect_timeout(&SystemEnv);
        let nats_client = mcp_nats::nats::connect(mcp.nats(), nats_connect_timeout).await?;
        let client_ids = ClientIdFactory::new(client_id_prefix);
        let http_config = streamable_http_config(allowed_hosts);
        let service = streamable_http_service(nats_client, mcp, client_ids, server_id, http_config);
        let app = Router::new().route_service(path.as_str(), service);
        let listener = TcpListener::bind(bind_addr).await?;

        info!(endpoint = %format!("http://{bind_addr}{}", path.as_str()), "MCP HTTP bridge starting");

        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await?;

        info!("MCP HTTP bridge stopped");

        Ok(())
    }

    fn init_logging() {
        let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
        let _ = tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_writer(std::io::stderr)
            .try_init();
    }

    fn streamable_http_config(allowed_hosts: Vec<String>) -> StreamableHttpServerConfig {
        let config = StreamableHttpServerConfig::default();
        if allowed_hosts.is_empty() {
            config
        } else {
            config.with_allowed_hosts(allowed_hosts)
        }
    }

    async fn shutdown_signal() {
        #[cfg(unix)]
        {
            let mut terminate = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(signal) => Some(signal),
                Err(error) => {
                    error!(error = %error, "Failed to listen for SIGTERM");
                    None
                }
            };

            tokio::select! {
                result = tokio::signal::ctrl_c() => {
                    if let Err(error) = result {
                        error!(error = %error, "Failed to listen for shutdown signal");
                    }
                }
                _ = async {
                    if let Some(signal) = terminate.as_mut() {
                        signal.recv().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {}
            }
        }

        #[cfg(not(unix))]
        {
            if let Err(error) = tokio::signal::ctrl_c().await {
                error!(error = %error, "Failed to listen for shutdown signal");
            }
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
        let mut peer = None;
        let mut pending = HashMap::new();

        loop {
            tokio::select! {
                command = command_rx.recv() => {
                    let Some(command) = command else {
                        break;
                    };
                    handle_proxy_command(command, &mut transport, &mut peer, &mut pending).await;
                }
                message = transport.receive() => {
                    let Some(message) = message else {
                        fail_pending(pending, ErrorData::internal_error("MCP NATS transport closed", None));
                        break;
                    };
                    handle_remote_message(message, &mut transport, peer.as_ref(), &mut pending).await;
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
        pending: &mut HashMap<RequestId, ProxyResponse>,
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
                pending.insert(request_id.clone(), response_tx);
                if let Err(error) = transport.send(message).await
                    && let Some(response_tx) = pending.remove(&request_id)
                {
                    let _ = response_tx.send(Err(ErrorData::internal_error(error.to_string(), None)));
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
        pending: &mut HashMap<RequestId, ProxyResponse>,
    ) where
        N: SubscribeClient + RequestClient + PublishClient + FlushClient,
        N::RequestError: 'static,
        N::PublishError: 'static,
        N::FlushError: 'static,
    {
        match message {
            ServerJsonRpcMessage::Response(response) => {
                if let Some(response_tx) = pending.remove(&response.id) {
                    let _ = response_tx.send(Ok(response.result));
                }
            }
            ServerJsonRpcMessage::Error(error) => {
                if let Some(response_tx) = pending.remove(&error.id) {
                    let _ = response_tx.send(Err(error.error));
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

    fn fail_pending(pending: HashMap<RequestId, ProxyResponse>, error: ErrorData) {
        for response_tx in pending.into_values() {
            let _ = response_tx.send(Err(error.clone()));
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
    mod tests {
        use axum::body::{Body, to_bytes};
        use axum::http::{Request, StatusCode, header};
        use rmcp::model::{
            ClientCapabilities, ClientInfo, ClientRequest, Implementation, InitializeRequest, InitializeRequestParams,
            InitializeResult, JsonRpcMessage, NumberOrString, ServerCapabilities, ServerResult,
        };
        use tower::ServiceExt;

        use super::*;

        fn mcp_config() -> Config {
            Config::new(
                mcp_nats::McpPrefix::new("mcp").unwrap(),
                trogon_nats::NatsConfig {
                    servers: vec!["localhost:4222".to_string()],
                    auth: trogon_nats::NatsAuth::None,
                },
            )
        }

        fn initialize_request() -> ClientJsonRpcMessage {
            ClientJsonRpcMessage::request(
                ClientRequest::InitializeRequest(InitializeRequest::new(InitializeRequestParams::new(
                    ClientCapabilities::default(),
                    Implementation::new("test-client", "1.0.0"),
                ))),
                NumberOrString::Number(1),
            )
        }

        fn initialize_response() -> ServerJsonRpcMessage {
            ServerJsonRpcMessage::response(
                ServerResult::InitializeResult(
                    InitializeResult::new(ServerCapabilities::default())
                        .with_server_info(Implementation::new("remote-server", "1.0.0")),
                ),
                NumberOrString::Number(1),
            )
        }

        #[tokio::test]
        async fn streamable_http_service_routes_initialize_to_nats_server() {
            let nats = trogon_nats::AdvancedMockNatsClient::new();
            let _inbound = nats.inject_messages();
            nats.set_response(
                "mcp.server.default.initialize",
                serde_json::to_vec(&initialize_response()).unwrap().into(),
            );
            let service = streamable_http_service(
                nats.clone(),
                mcp_config(),
                ClientIdFactory::new(McpPeerId::new("http").unwrap()),
                McpPeerId::new("default").unwrap(),
                StreamableHttpServerConfig::default(),
            );
            let app = Router::new().route_service("/mcp", service);
            let body = serde_json::to_vec(&initialize_request()).unwrap();

            let response = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/mcp")
                        .header(header::HOST, "localhost")
                        .header(header::ACCEPT, "application/json, text/event-stream")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::OK);
            assert!(response.headers().contains_key("mcp-session-id"));
            assert_eq!(nats.subscribed_to().len(), 1);
            assert!(nats.subscribed_to()[0].starts_with("mcp.client.http-"));
            assert!(nats.subscribed_to()[0].ends_with(".>"));
            let response_body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let body = String::from_utf8(response_body.to_vec()).unwrap();
            assert!(body.contains("remote-server"));
        }

        #[test]
        fn client_id_factory_generates_valid_unique_peer_ids() {
            let factory = ClientIdFactory::new(McpPeerId::new("http").unwrap());

            let first = factory.next().unwrap();
            let second = factory.next().unwrap();

            assert!(first.as_str().starts_with("http-"));
            assert!(second.as_str().starts_with("http-"));
            assert_ne!(first, second);
        }

        #[test]
        fn streamable_http_config_uses_sdk_defaults_unless_allowed_hosts_are_overridden() {
            assert_eq!(
                streamable_http_config(Vec::new()).allowed_hosts,
                StreamableHttpServerConfig::default().allowed_hosts
            );
            assert_eq!(
                streamable_http_config(vec!["example.com".to_string()]).allowed_hosts,
                vec!["example.com"]
            );
        }

        #[tokio::test]
        async fn service_info_is_available_before_remote_initialize() {
            let nats = trogon_nats::AdvancedMockNatsClient::new();
            let service = McpNatsProxyService::new(
                nats,
                mcp_config(),
                McpPeerId::new("http-test").unwrap(),
                McpPeerId::new("default").unwrap(),
            );

            let info: ServerInfo = service.get_info();

            assert!(!info.server_info.name.is_empty());
        }

        #[test]
        fn initialize_request_uses_client_info_type() {
            let message = initialize_request();

            let JsonRpcMessage::Request(request) = message else {
                panic!("expected initialize request");
            };
            let ClientRequest::InitializeRequest(InitializeRequest { params, .. }) = request.request else {
                panic!("expected initialize method");
            };
            let _: ClientInfo = params;
        }
    }
}

#[cfg(not(coverage))]
fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    runtime::main()
}

#[cfg(coverage)]
fn main() {}

#[cfg(all(test, coverage))]
mod coverage_tests {
    #[test]
    fn coverage_main_stub_is_callable() {
        super::main();
    }
}
