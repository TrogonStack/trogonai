use a2a::types::{ListTasksRequest, ListTasksResponse};
        use bytes::Bytes;
        use trogon_nats::AdvancedMockNatsClient;

        use super::*;

        fn list_tasks_request() -> ListTasksRequest {
            ListTasksRequest {
                context_id: None,
                status: None,
                page_size: None,
                page_token: None,
                history_length: None,
                status_timestamp_after: None,
                include_artifacts: None,
                tenant: None,
            }
        }

        fn list_response() -> Bytes {
            let response = ListTasksResponse {
                tasks: vec![],
                next_page_token: String::new(),
                page_size: 0,
                total_size: 0,
            };
            let json = serde_json::json!({"jsonrpc":"2.0","id":"any","result":response});
            serde_json::to_vec(&json).unwrap().into()
        }

        fn error_response(code: i32, msg: &str) -> Bytes {
            let json = serde_json::json!({"jsonrpc":"2.0","id":"any","error":{"code":code,"message":msg}});
            serde_json::to_vec(&json).unwrap().into()
        }

        #[tokio::test]
        async fn tasks_list_targets_agent_subject_by_default() {
            let nats = AdvancedMockNatsClient::new();
            nats.set_response("a2a.agents.test-agent.tasks.list", list_response());
            let client = A2aClient::new(prefix(), agent_id(), nats, ());
            let resp = client.tasks_list(&list_tasks_request()).await.unwrap();
            assert!(resp.tasks.is_empty());
        }

        #[tokio::test]
        async fn tasks_list_targets_gateway_subject_under_gateway_routing() {
            let nats = AdvancedMockNatsClient::new();
            nats.set_response("a2a.gateway.test-agent.tasks.list", list_response());
            let jwt =
                MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjk5OTk5OTk5OTl9.signature").unwrap();
            let client = A2aClient::new(prefix(), agent_id(), nats, ()).routing_via_gateway_ingress(jwt);
            client.tasks_list(&list_tasks_request()).await.unwrap();
        }

        #[tokio::test]
        async fn tasks_list_propagates_typed_jsonrpc_errors() {
            let nats = AdvancedMockNatsClient::new();
            nats.set_response("a2a.agents.test-agent.tasks.list", error_response(-32050, "down"));
            let client = A2aClient::new(prefix(), agent_id(), nats, ());
            let err = client.tasks_list(&list_tasks_request()).await.unwrap_err();
            assert!(matches!(err, ClientError::AgentUnavailable));
        }

        #[tokio::test]
        async fn tasks_list_propagates_transport_errors() {
            let nats = AdvancedMockNatsClient::new();
            nats.fail_next_request();
            let client = A2aClient::new(prefix(), agent_id(), nats, ());
            assert!(matches!(
                client.tasks_list(&list_tasks_request()).await,
                Err(ClientError::Transport(_))
            ));
        }
