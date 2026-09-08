use std::time::Duration;

use jsonrpc_nats::RequestId;
use serde::{Serialize, de::DeserializeOwned};
use trogon_nats::RequestClient;

use a2a_identity_types::MintedUserJwt;

use crate::req_id::ReqId;

use super::error::ClientError;
use super::gateway_headers::{agent_rpc_headers, gateway_ingress_rpc_headers};
use super::validated::ValidatedRpc;
use super::wire::{decode_client_response, encode_client_request, map_wire_error, merge_jsonrpc_headers};

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
    Ok(
        send_unary_validated(nats, subject, method, params, req_id, timeout, gateway_caller_jwt)
            .await?
            .value,
    )
}

/// Like [`send_unary`], but retains the validated canonical response body for
/// edge bridges that forward the envelope unmodified (ADR#0056).
pub async fn send_unary_validated<N, Req, Res>(
    nats: &N,
    subject: &str,
    method: &'static str,
    params: &Req,
    req_id: &ReqId,
    timeout: Duration,
    gateway_caller_jwt: Option<&MintedUserJwt>,
) -> Result<ValidatedRpc<Res>, ClientError>
where
    N: RequestClient,
    Req: Serialize,
    Res: DeserializeOwned,
{
    let encoded =
        encode_client_request(method, RequestId::String(req_id.as_str().to_owned()), params).map_err(map_wire_error)?;

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
    let body = msg.payload.clone();
    match decode_client_response::<Res>(&response_headers, &body).map_err(map_wire_error)? {
        Ok(result) => Ok(ValidatedRpc::new(result, body)),
        Err((code, message)) => Err(ClientError::from_jsonrpc_code(code, message)),
    }
}

#[cfg(test)]
mod tests;
