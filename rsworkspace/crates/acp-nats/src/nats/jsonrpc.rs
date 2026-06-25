use std::time::Duration;

use agent_client_protocol::Error;
use jsonrpc_nats::{
    Message, RequestId, TransportError, jsonrpc_request_with_timeout as shared_jsonrpc_request_with_timeout,
};
use serde::{Serialize, de::DeserializeOwned};
use trogon_nats::{NatsError, RequestClient, build_request_headers};

use crate::nats::markers::Requestable;
use crate::wire::WireError;

/// JSON-RPC content-mode request/reply over core NATS.
pub async fn jsonrpc_request_with_timeout<N, Req, Res>(
    client: &N,
    subject: &impl Requestable,
    method: &str,
    request: &Req,
    timeout: Duration,
) -> Result<Res, JsonRpcRequestError>
where
    N: RequestClient,
    Req: Serialize,
    Res: DeserializeOwned,
    N::RequestError: 'static,
{
    let subject = subject.to_string();
    let request_id = RequestId::String(uuid::Uuid::new_v4().to_string());
    match shared_jsonrpc_request_with_timeout(
        client,
        &subject,
        method,
        request_id,
        request,
        build_request_headers(),
        timeout,
    )
    .await
    {
        Ok(Message::Success { result, .. }) => serde_json::from_value(result)
            .map_err(WireError::Deserialize)
            .map_err(JsonRpcRequestError::Wire),
        Ok(Message::Error {
            code, message, data: _, ..
        }) => Err(JsonRpcRequestError::Agent(Error::new(code, message))),
        Ok(_) => Err(JsonRpcRequestError::Wire(WireError::UnexpectedMessage)),
        Err(error) => Err(map_transport_error(error, &subject)),
    }
}

fn map_transport_error(error: TransportError, _subject: &str) -> JsonRpcRequestError {
    match error {
        TransportError::Codec(error) => JsonRpcRequestError::Wire(WireError::Codec(error)),
        TransportError::UnexpectedResponse => JsonRpcRequestError::Wire(WireError::UnexpectedMessage),
        TransportError::Timeout { subject } => JsonRpcRequestError::Nats(NatsError::Timeout { subject }),
        TransportError::Request { subject, error } => JsonRpcRequestError::Nats(NatsError::Request { subject, error }),
        TransportError::Publish { subject, error } => {
            JsonRpcRequestError::Nats(NatsError::Other(format!("{subject}: {error}")))
        }
        TransportError::Flush { error } => JsonRpcRequestError::Nats(NatsError::Other(error)),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum JsonRpcRequestError {
    #[error(transparent)]
    Nats(#[from] NatsError),
    #[error(transparent)]
    Wire(#[from] WireError),
    #[error("agent returned a structured error")]
    Agent(#[from] agent_client_protocol::Error),
}

impl JsonRpcRequestError {
    pub fn into_nats_error(self, subject: &str) -> NatsError {
        match self {
            Self::Nats(error) => error,
            Self::Wire(error) => NatsError::Other(format!("{subject}: {error}")),
            Self::Agent(error) => {
                NatsError::Other(format!("agent error {} on {subject}: {}", error.code, error.message))
            }
        }
    }
}
