use crate::agent::test_support::{has_request_metric, mock_bridge, mock_bridge_with_metrics, set_js_response};
    use agent_client_protocol::{Agent, CloseSessionRequest, CloseSessionResponse, ErrorCode};

    #[tokio::test]
    async fn close_session_forwards_request_and_returns_response() {
        let (_mock, js, bridge) = mock_bridge();
        let expected = CloseSessionResponse::new();
        set_js_response(&js, &expected);

        let request = CloseSessionRequest::new("s1");
        let result = bridge.close_session(request).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn close_session_returns_error_when_js_fails() {
        let (_mock, _js, bridge) = mock_bridge();

        let request = CloseSessionRequest::new("s1");
        let err = bridge.close_session(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn close_session_returns_error_when_response_is_invalid_json() {
        let (_mock, js, bridge) = mock_bridge();
        crate::agent::test_support::set_js_raw_response(&js, b"not json");

        let request = CloseSessionRequest::new("s1");
        let err = bridge.close_session(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn close_session_validates_session_id() {
        let (_mock, _js, bridge) = mock_bridge();
        let request = CloseSessionRequest::new("invalid.session.id");
        let err = bridge.close_session(request).await.unwrap_err();

        assert!(err.to_string().contains("Invalid session ID"));
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn close_session_records_metrics_on_success() {
        let (_mock, js, bridge, exporter, provider) = mock_bridge_with_metrics();
        set_js_response(&js, &CloseSessionResponse::new());

        let _ = bridge.close_session(CloseSessionRequest::new("s1")).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "close_session", true),
            "expected acp.requests with method=close_session, success=true"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn close_session_records_metrics_on_failure() {
        let (_mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();

        let _ = bridge.close_session(CloseSessionRequest::new("s1")).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "close_session", false),
            "expected acp.requests with method=close_session, success=false"
        );
        provider.shutdown().unwrap();
    }
