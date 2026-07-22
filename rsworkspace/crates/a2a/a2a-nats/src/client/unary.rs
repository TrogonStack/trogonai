use std::time::Duration;

use serde::{Serialize, de::DeserializeOwned};
use trogon_nats::RequestClient;

use a2a_identity_types::MintedUserJwt;

use crate::jsonrpc::JsonRpcId;
use crate::req_id::ReqId;

use super::error::ClientError;
use super::gateway_headers::{agent_rpc_headers, gateway_ingress_rpc_headers};
use super::wire::{decode_client_response, encode_client_request, merge_jsonrpc_headers};

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
    let encoded = encode_client_request(method, JsonRpcId::String(req_id.as_str().to_owned()), params)
        .map_err(|e| ClientError::Serialize(<serde_json::Error as serde::de::Error>::custom(format!("{e}"))))?;

    let headers = match gateway_caller_jwt {
        Some(jwt) => gateway_ingress_rpc_headers(req_id, jwt)?,
        None => agent_rpc_headers(req_id),
    };
    let headers = merge_jsonrpc_headers(headers, encoded.headers);

    let msg = tokio::time::timeout(
        timeout,
        nats.request_with_headers(subject.to_string(), headers, encoded.body),
    )
    .await
    .map_err(|_| ClientError::Timeout {
        subject: subject.to_string(),
    })?
    .map_err(|e| ClientError::Transport(e.to_string()))?;

    let response_headers = msg.headers.unwrap_or_default();
    match decode_client_response::<Res>(&response_headers, &msg.payload).map_err(map_wire_error)? {
        Ok(result) => Ok(result),
        Err((code, message)) => Err(ClientError::from_jsonrpc_code(code, message)),
    }
}

fn map_wire_error(error: crate::wire::WireError) -> ClientError {
    match error {
        crate::wire::WireError::Deserialize(e) => ClientError::Deserialize(e),
        other => ClientError::Deserialize(<serde_json::Error as serde::de::Error>::custom(format!("{other}"))),
    }
}

#[cfg(test)]
mod tests;
