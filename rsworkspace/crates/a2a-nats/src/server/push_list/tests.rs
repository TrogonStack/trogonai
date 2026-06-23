    use trogon_nats::AdvancedMockNatsClient;

    use super::*;
    use crate::server::test_support::{parse_response, stub};

    fn list_request() -> a2a::types::ListTaskPushNotificationConfigsRequest {
        a2a::types::ListTaskPushNotificationConfigsRequest {
            task_id: "task-1".to_string(),
            page_size: None,
            page_token: None,
            tenant: None,
        }
    }

    fn empty_response() -> a2a::types::ListTaskPushNotificationConfigsResponse {
        a2a::types::ListTaskPushNotificationConfigsResponse {
            configs: vec![],
            next_page_token: None,
        }
    }

    fn list_payload(req_id: i64) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": "tasks/pushNotificationConfig/list",
            "params": list_request()
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn success_publishes_list() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().push_list_result = Some(Ok(empty_response()));
        handle(&handler, &list_payload(1), Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        // configs is skip_serializing_if=Vec::is_empty, so an empty list omits the field.
        assert!(body.get("result").is_some());
        assert!(body["error"].is_null());
    }

    #[tokio::test]
    async fn push_not_supported_error_uses_typed_code() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().push_list_result = Some(Err(A2aError::push_notification_not_supported("no push")));
        handle(&handler, &list_payload(2), Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(
            body["error"]["code"].as_i64(),
            Some(i64::from(crate::error::PUSH_NOTIFICATION_NOT_SUPPORTED))
        );
    }

    #[tokio::test]
    async fn no_reply_drops_request() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handle(&handler, &list_payload(3), None, &nats).await;
        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn missing_params_returns_invalid_params_error() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        let payload = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tasks/pushNotificationConfig/list"
        }))
        .unwrap();
        handle(&handler, &payload, Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn invalid_params_shape_returns_invalid_params_code() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        let payload = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "tasks/pushNotificationConfig/list",
            "params": { "taskId": 42 }
        }))
        .unwrap();
        handle(&handler, &payload, Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["error"]["code"], -32602);
        assert_eq!(body["id"], 6);
    }

    #[tokio::test]
    async fn malformed_json_still_publishes_parse_error_with_null_id() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handle(&handler, b"not json", Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["error"]["code"], -32700);
        assert!(body["id"].is_null());
    }

    #[tokio::test]
    async fn notification_without_id_is_dropped() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        let payload = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "tasks/pushNotificationConfig/list",
            "params": list_request()
        }))
        .unwrap();
        handle(&handler, &payload, Some("r".into()), &nats).await;
        assert!(nats.published_messages().is_empty());
    }
