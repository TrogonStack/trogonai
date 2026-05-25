use bytes::Bytes;
use serde::{Serialize, de::DeserializeOwned};
use std::time::Duration;
use trogon_nats::RequestClient;

use a2a_auth_callout::MintedUserJwt;

use crate::jsonrpc::JsonRpcId;
use crate::req_id::ReqId;

use super::error::ClientError;
use super::gateway_headers::{agent_rpc_headers, gateway_ingress_rpc_headers};
use super::wire::{JsonRpcRequest, JsonRpcResponse};

pub async fn send_unary<N, Req, Res>(
    nats: &N,
    subject: &str,
    method: &'static str,
    params: &Req,
    req_id: &ReqId,
    timeout: Duration,
    gateway_caller_jwt: Option<&MintedUserJwt>,
) -> Result<Res, ClientError>
where
    N: RequestClient,
    Req: Serialize,
    Res: DeserializeOwned,
{
    let envelope = JsonRpcRequest::new(JsonRpcId::String(req_id.as_str().to_owned()), method, params);
    let payload = serde_json::to_vec(&envelope).map_err(ClientError::Serialize)?;

    let headers = match gateway_caller_jwt {
        Some(jwt) => gateway_ingress_rpc_headers(req_id, jwt)?,
        None => agent_rpc_headers(req_id),
    };

    let msg = tokio::time::timeout(
        timeout,
        nats.request_with_headers(subject.to_string(), headers, Bytes::from(payload)),
    )
    .await
    .map_err(|_| ClientError::Timeout {
        subject: subject.to_string(),
    })?
    .map_err(|e| ClientError::Transport(e.to_string()))?;

    let response: JsonRpcResponse<Res> = serde_json::from_slice(&msg.payload).map_err(ClientError::Deserialize)?;

    match response {
        JsonRpcResponse::Success(s) => Ok(s.result),
        JsonRpcResponse::Error(e) => Err(ClientError::from_jsonrpc_code(e.error.code, e.error.message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use trogon_nats::AdvancedMockNatsClient;

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct Params {
        x: i32,
    }

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct Response {
        y: String,
    }

    fn req_id() -> ReqId {
        ReqId::from_test("test-req-1")
    }

    fn success_response(y: &str) -> bytes::Bytes {
        let json = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "test-req-1",
            "result": { "y": y }
        });
        serde_json::to_vec(&json).unwrap().into()
    }

    fn error_response(code: i32, message: &str) -> bytes::Bytes {
        let json = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "test-req-1",
            "error": { "code": code, "message": message }
        });
        serde_json::to_vec(&json).unwrap().into()
    }

    #[tokio::test]
    async fn success_response_deserializes_result() {
        let mock = AdvancedMockNatsClient::new();
        mock.set_response("a2a.agent.bot.tasks.get", success_response("hello"));

        let result: Result<Response, _> = send_unary(
            &mock,
            "a2a.agent.bot.tasks.get",
            "tasks/get",
            &Params { x: 1 },
            &req_id(),
            Duration::from_secs(5),
            None,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().y, "hello");
    }

    #[tokio::test]
    async fn task_not_found_error_code_maps_to_typed_error() {
        let mock = AdvancedMockNatsClient::new();
        mock.set_response("a2a.agent.bot.tasks.get", error_response(-32001, "Task not found"));

        let result: Result<Response, _> = send_unary(
            &mock,
            "a2a.agent.bot.tasks.get",
            "tasks/get",
            &Params { x: 1 },
            &req_id(),
            Duration::from_secs(5),
            None,
        )
        .await;

        assert!(matches!(result, Err(ClientError::TaskNotFound)));
    }

    #[tokio::test]
    async fn agent_unavailable_code_maps_to_typed_error() {
        let mock = AdvancedMockNatsClient::new();
        mock.set_response("a2a.agent.bot.tasks.get", error_response(-32050, "no responders"));

        let result: Result<Response, _> = send_unary(
            &mock,
            "a2a.agent.bot.tasks.get",
            "tasks/get",
            &Params { x: 1 },
            &req_id(),
            Duration::from_secs(5),
            None,
        )
        .await;

        assert!(matches!(result, Err(ClientError::AgentUnavailable)));
    }

    #[tokio::test]
    async fn transport_failure_returns_transport_error() {
        let mock = AdvancedMockNatsClient::new();
        mock.fail_next_request();

        let result: Result<Response, _> = send_unary(
            &mock,
            "a2a.agent.bot.tasks.get",
            "tasks/get",
            &Params { x: 1 },
            &req_id(),
            Duration::from_secs(5),
            None,
        )
        .await;

        assert!(matches!(result, Err(ClientError::Transport(_))));
    }

    #[tokio::test]
    async fn hang_returns_timeout_error() {
        let mock = AdvancedMockNatsClient::new();
        mock.hang_next_request();

        let result: Result<Response, _> = send_unary(
            &mock,
            "a2a.agent.bot.tasks.get",
            "tasks/get",
            &Params { x: 1 },
            &req_id(),
            Duration::from_millis(10),
            None,
        )
        .await;

        assert!(matches!(result, Err(ClientError::Timeout { .. })));
    }

    #[tokio::test]
    async fn malformed_response_returns_deserialize_error() {
        let mock = AdvancedMockNatsClient::new();
        mock.set_response("a2a.agent.bot.tasks.get", b"not json at all".as_ref().into());

        let result: Result<Response, _> = send_unary(
            &mock,
            "a2a.agent.bot.tasks.get",
            "tasks/get",
            &Params { x: 1 },
            &req_id(),
            Duration::from_secs(5),
            None,
        )
        .await;

        assert!(matches!(result, Err(ClientError::Deserialize(_))));
    }

    #[tokio::test]
    async fn unknown_error_code_maps_to_generic_jsonrpc_error() {
        let mock = AdvancedMockNatsClient::new();
        mock.set_response("a2a.agent.bot.tasks.get", error_response(-32099, "custom"));

        let result: Result<Response, _> = send_unary(
            &mock,
            "a2a.agent.bot.tasks.get",
            "tasks/get",
            &Params { x: 1 },
            &req_id(),
            Duration::from_secs(5),
            None,
        )
        .await;

        assert!(matches!(result, Err(ClientError::JsonRpc { code: -32099, .. })));
    }
}
