use super::Bridge;
use crate::error::map_nats_error;
use crate::ext_method_name::ExtMethodName;
use crate::nats::{self, RequestClient, agent};
use agent_client_protocol::{Error, ErrorCode, ExtRequest, ExtResponse, Result};
use tracing::{info, instrument};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.ext",
    skip(bridge, args),
    fields(method = %args.method)
)]
pub async fn handle<N: RequestClient, C: GetElapsed, J>(
    bridge: &Bridge<N, C, J>,
    args: ExtRequest,
) -> Result<ExtResponse> {
    let start = bridge.clock.now();

    info!(method = %args.method, "Extension method request");

    let method_name = ExtMethodName::new(&args.method).map_err(|e| {
        bridge
            .metrics
            .record_request("ext_method", bridge.clock.elapsed(start).as_secs_f64(), false);
        bridge.metrics.record_error("ext_method", "invalid_method_name");
        Error::new(ErrorCode::InvalidParams.into(), format!("Invalid method name: {}", e))
    })?;

    let nats = bridge.nats();
    let subject = agent::ExtSubject::new(bridge.config.acp_prefix_ref(), &method_name);

    let result = nats::request_with_timeout::<N, ExtRequest, serde_json::Value>(
        nats,
        &subject,
        &args,
        bridge.config.operation_timeout(),
    )
    .await
    .map_err(map_nats_error)
    .and_then(discriminate_envelope);

    bridge
        .metrics
        .record_request("ext_method", bridge.clock.elapsed(start).as_secs_f64(), result.is_ok());

    result
}

/// Discriminate between the new discriminated envelope and the old bare format.
///
/// New format (sent by updated runners):
///   - Success: `{"result": <ExtResponse body>}`
///   - Error:   `{"error": {"code": <int>, "message": "<str>", "data": <null|...>}}`
///
/// Old bare format (compat, sent by legacy runners):
///   - Success: the ExtResponse body directly
///   - Error:   `{"code": <int>, "message": "<str>"}` (bare JSON-RPC error object)
fn discriminate_envelope(env: serde_json::Value) -> Result<ExtResponse> {
    // New envelope: {"error": {...}}
    if let Some(err_val) = env.get("error") {
        let code = err_val.get("code").and_then(|c| c.as_i64()).unwrap_or(-32603) as i32;
        let message = err_val
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("ext method error")
            .to_string();
        return Err(Error::new(code, message));
    }

    // New envelope: {"result": <body>}
    if let Some(result_val) = env.get("result") {
        let raw = serde_json::value::RawValue::from_string(result_val.to_string())
            .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("Invalid ext response body: {e}")))?;
        return Ok(ExtResponse::new(raw.into()));
    }

    // Compat: old bare error — {"code": <int>, "message": "<str>"}
    if let (Some(code_val), Some(message)) = (env.get("code"), env.get("message").and_then(|m| m.as_str())) {
        let code = code_val.as_i64().unwrap_or(-32603) as i32;
        return Err(Error::new(code, message.to_string()));
    }

    // Compat: old bare success — ExtResponse body directly
    let raw = serde_json::value::RawValue::from_string(env.to_string())
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("Invalid ext response: {e}")))?;
    Ok(ExtResponse::new(raw.into()))
}

#[cfg(test)]
mod tests {
    use crate::agent::test_support::{
        has_error_metric, has_request_metric, mock_bridge, mock_bridge_with_metrics, set_json_response,
    };
    use agent_client_protocol::{Agent, ErrorCode, ExtRequest, ExtResponse};
    use serde_json::value::RawValue;

    // ── new discriminated envelope ────────────────────────────────────────────

    #[tokio::test]
    async fn ext_method_new_envelope_success_returns_ok() {
        let (mock, _js, bridge) = mock_bridge();
        // Runner sends {"result": <body>}
        mock.set_response(
            "acp.agent.ext.my_method",
            br#"{"result":{"answer":42}}"#.to_vec().into(),
        );

        let params = RawValue::from_string("{}".to_string()).unwrap();
        let result = bridge.ext_method(ExtRequest::new("my_method", params.into())).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn ext_method_new_envelope_error_surfaces() {
        let (mock, _js, bridge) = mock_bridge();
        // Runner sends {"error": {"code": -32001, "message": "runner failed"}}
        mock.set_response(
            "acp.agent.ext.my_method",
            br#"{"error":{"code":-32001,"message":"runner failed"}}"#.to_vec().into(),
        );

        let params = RawValue::from_string("{}".to_string()).unwrap();
        let err = bridge.ext_method(ExtRequest::new("my_method", params.into())).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::Other(-32001));
        assert_eq!(err.message, "runner failed");
    }

    #[tokio::test]
    async fn ext_method_new_envelope_error_uses_default_code_when_missing() {
        let (mock, _js, bridge) = mock_bridge();
        // Malformed error object — no code field
        mock.set_response(
            "acp.agent.ext.my_method",
            br#"{"error":{"message":"oops"}}"#.to_vec().into(),
        );

        let params = RawValue::from_string("{}".to_string()).unwrap();
        let err = bridge.ext_method(ExtRequest::new("my_method", params.into())).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::InternalError); // -32603 default
        assert_eq!(err.message, "oops");
    }

    // ── compat: old bare format ───────────────────────────────────────────────

    #[tokio::test]
    async fn ext_method_compat_bare_success_returns_ok() {
        let (mock, _js, bridge) = mock_bridge();
        // Old runner sends the ExtResponse body directly (not wrapped in {"result":...})
        let raw = RawValue::from_string(r#"{"answer":42}"#.to_string()).unwrap();
        let expected = ExtResponse::new(raw.into());
        set_json_response(&mock, "acp.agent.ext.my_method", &expected);

        let params = RawValue::from_string(r#"{"key":"value"}"#.to_string()).unwrap();
        let result = bridge.ext_method(ExtRequest::new("my_method", params.into())).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn ext_method_compat_bare_error_surfaces() {
        let (mock, _js, bridge) = mock_bridge();
        // Old runner sends a bare error: {"code": -32603, "message": "old error"}
        mock.set_response(
            "acp.agent.ext.my_method",
            br#"{"code":-32603,"message":"old error"}"#.to_vec().into(),
        );

        let params = RawValue::from_string("{}".to_string()).unwrap();
        let err = bridge.ext_method(ExtRequest::new("my_method", params.into())).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::InternalError);
        assert_eq!(err.message, "old error");
    }

    // ── original test kept for regression ────────────────────────────────────

    #[tokio::test]
    async fn ext_method_forwards_request_and_returns_response() {
        let (mock, _js, bridge) = mock_bridge();
        let raw = RawValue::from_string(r#"{"result":"ok"}"#.to_string()).unwrap();
        let expected = ExtResponse::new(raw.into());
        set_json_response(&mock, "acp.agent.ext.my_method", &expected);

        let params = RawValue::from_string(r#"{"key":"value"}"#.to_string()).unwrap();
        let request = ExtRequest::new("my_method", params.into());
        let result = bridge.ext_method(request).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn ext_method_returns_error_when_nats_fails() {
        let (mock, _js, bridge) = mock_bridge();
        mock.fail_next_request();

        let params = RawValue::from_string("{}".to_string()).unwrap();
        let request = ExtRequest::new("my_method", params.into());
        let err = bridge.ext_method(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::Other(crate::error::AGENT_UNAVAILABLE));
    }

    #[tokio::test]
    async fn ext_method_returns_error_when_response_is_invalid_json() {
        let (mock, _js, bridge) = mock_bridge();
        mock.set_response("acp.agent.ext.my_method", "not json".into());

        let params = RawValue::from_string("{}".to_string()).unwrap();
        let request = ExtRequest::new("my_method", params.into());
        let err = bridge.ext_method(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn ext_method_validates_method_name() {
        let (_mock, _js, bridge) = mock_bridge();
        let params = RawValue::from_string("{}".to_string()).unwrap();
        let request = ExtRequest::new("method.*", params.into());
        let err = bridge.ext_method(request).await.unwrap_err();

        assert!(err.to_string().contains("Invalid method name"));
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn ext_method_records_error_metric_on_invalid_method_name() {
        let (_mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();
        let params = RawValue::from_string("{}".to_string()).unwrap();
        let request = ExtRequest::new("invalid method", params.into());

        let _ = bridge.ext_method(request).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_error_metric(&finished_metrics, "ext_method", "invalid_method_name"),
            "expected acp.errors with operation=ext_method, reason=invalid_method_name"
        );
        assert!(
            has_request_metric(&finished_metrics, "ext_method", false),
            "expected acp.requests with method=ext_method, success=false on validation failure"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn ext_method_records_metrics_on_success() {
        let (mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();
        let raw = RawValue::from_string("{}".to_string()).unwrap();
        set_json_response(&mock, "acp.agent.ext.my_method", &ExtResponse::new(raw.into()));

        let params = RawValue::from_string("{}".to_string()).unwrap();
        let _ = bridge.ext_method(ExtRequest::new("my_method", params.into())).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "ext_method", true),
            "expected acp.requests with method=ext_method, success=true"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn ext_method_records_metrics_on_new_envelope_error() {
        let (mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();
        mock.set_response(
            "acp.agent.ext.my_method",
            br#"{"error":{"code":-32001,"message":"runner failed"}}"#.to_vec().into(),
        );

        let params = RawValue::from_string("{}".to_string()).unwrap();
        let _ = bridge.ext_method(ExtRequest::new("my_method", params.into())).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "ext_method", false),
            "expected acp.requests with method=ext_method, success=false on envelope error"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn ext_method_records_metrics_on_failure() {
        let (mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();
        mock.fail_next_request();

        let params = RawValue::from_string("{}".to_string()).unwrap();
        let _ = bridge.ext_method(ExtRequest::new("my_method", params.into())).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "ext_method", false),
            "expected acp.requests with method=ext_method, success=false"
        );
        provider.shutdown().unwrap();
    }
}
