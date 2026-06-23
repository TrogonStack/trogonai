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
