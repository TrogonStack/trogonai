use std::time::Duration;

use bytes::Bytes;
use serde::{Serialize, de::DeserializeOwned};
use trogon_nats::RequestClient;

use a2a_identity_types::MintedUserJwt;

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
mod tests;
