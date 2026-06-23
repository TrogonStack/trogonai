use crate::agent::test_support::{has_request_metric, mock_bridge, mock_bridge_with_metrics, set_js_response};
    use agent_client_protocol::{Agent, ErrorCode, SetSessionModelRequest, SetSessionModelResponse};

    #[tokio::test]
    async fn set_session_model_forwards_request_and_returns_response() {
        let (_mock, js, bridge) = mock_bridge();
        let expected = SetSessionModelResponse::new();
        set_js_response(&js, &expected);

        let request = SetSessionModelRequest::new("s1", "claude-sonnet-4-6");
        let result = bridge.set_session_model(request).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn set_session_model_returns_error_when_js_fails() {
        let (_mock, _js, bridge) = mock_bridge();

        let request = SetSessionModelRequest::new("s1", "claude-sonnet-4-6");
        let err = bridge.set_session_model(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn set_session_model_returns_error_when_response_is_invalid_json() {
        let (_mock, js, bridge) = mock_bridge();
        crate::agent::test_support::set_js_raw_response(&js, b"not json");

        let request = SetSessionModelRequest::new("s1", "claude-sonnet-4-6");
        let err = bridge.set_session_model(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn set_session_model_validates_session_id() {
        let (_mock, _js, bridge) = mock_bridge();
        let request = SetSessionModelRequest::new("invalid.session.id", "claude-sonnet-4-6");
        let err = bridge.set_session_model(request).await.unwrap_err();

        assert!(err.to_string().contains("Invalid session ID"));
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn set_session_model_records_metrics_on_success() {
        let (_mock, js, bridge, exporter, provider) = mock_bridge_with_metrics();
        set_js_response(&js, &SetSessionModelResponse::new());

        let _ = bridge
            .set_session_model(SetSessionModelRequest::new("s1", "claude-sonnet-4-6"))
            .await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "set_session_model", true),
            "expected acp.requests with method=set_session_model, success=true"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn set_session_model_records_metrics_on_failure() {
        let (_mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();

        let _ = bridge
            .set_session_model(SetSessionModelRequest::new("s1", "claude-sonnet-4-6"))
            .await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "set_session_model", false),
            "expected acp.requests with method=set_session_model, success=false"
        );
        provider.shutdown().unwrap();
    }
