use a2a::types::DeleteTaskPushNotificationConfigRequest;
        use bytes::Bytes;
        use trogon_nats::AdvancedMockNatsClient;

        use super::*;

        fn delete_request(id: &str) -> DeleteTaskPushNotificationConfigRequest {
            DeleteTaskPushNotificationConfigRequest {
                task_id: "task-1".to_string(),
                id: id.to_string(),
                tenant: None,
            }
        }

        fn ok_response() -> Bytes {
            let json = serde_json::json!({"jsonrpc":"2.0","id":"any","result":null});
            serde_json::to_vec(&json).unwrap().into()
        }

        fn error_response(code: i32, msg: &str) -> Bytes {
            let json = serde_json::json!({"jsonrpc":"2.0","id":"any","error":{"code":code,"message":msg}});
            serde_json::to_vec(&json).unwrap().into()
        }

        #[tokio::test]
        async fn push_delete_targets_agent_subject_by_default() {
            let nats = AdvancedMockNatsClient::new();
            nats.set_response("a2a.agents.test-agent.push.delete", ok_response());
            let client = A2aClient::new(prefix(), agent_id(), nats, ());
            client.push_delete(&delete_request("c-1")).await.unwrap();
        }

        #[tokio::test]
        async fn push_delete_targets_gateway_subject_under_gateway_routing() {
            let nats = AdvancedMockNatsClient::new();
            nats.set_response("a2a.gateway.test-agent.push.delete", ok_response());
            let jwt =
                MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjk5OTk5OTk5OTl9.signature").unwrap();
            let client = A2aClient::new(prefix(), agent_id(), nats, ()).routing_via_gateway_ingress(jwt);
            client.push_delete(&delete_request("c-gw")).await.unwrap();
        }

        #[tokio::test]
        async fn push_delete_propagates_task_not_found() {
            let nats = AdvancedMockNatsClient::new();
            nats.set_response("a2a.agents.test-agent.push.delete", error_response(-32001, "missing"));
            let client = A2aClient::new(prefix(), agent_id(), nats, ());
            assert!(matches!(
                client.push_delete(&delete_request("c")).await,
                Err(ClientError::TaskNotFound)
            ));
        }

        #[tokio::test]
        async fn push_delete_propagates_transport_errors() {
            let nats = AdvancedMockNatsClient::new();
            nats.fail_next_request();
            let client = A2aClient::new(prefix(), agent_id(), nats, ());
            assert!(matches!(
                client.push_delete(&delete_request("c")).await,
                Err(ClientError::Transport(_))
            ));
        }
