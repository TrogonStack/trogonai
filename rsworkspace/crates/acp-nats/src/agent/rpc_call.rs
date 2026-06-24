use std::time::Duration;

use crate::error::map_nats_error;
use crate::nats::{RequestClient, jsonrpc::JsonRpcRequestError, markers::Requestable};
use agent_client_protocol::Result;

pub(crate) async fn jsonrpc_call<N, Req, Res>(
    nats: &N,
    subject: &impl Requestable,
    method: &str,
    request: &Req,
    timeout: Duration,
) -> Result<Res>
where
    N: RequestClient,
    Req: serde::Serialize,
    Res: serde::de::DeserializeOwned,
    N::RequestError: 'static,
{
    let subject_str = subject.to_string();
    match crate::nats::jsonrpc::jsonrpc_request_with_timeout(nats, subject, method, request, timeout).await {
        Ok(response) => Ok(response),
        Err(JsonRpcRequestError::Agent(error)) => Err(error),
        Err(JsonRpcRequestError::Wire(_)) => Err(agent_client_protocol::Error::new(
            agent_client_protocol::ErrorCode::InternalError.into(),
            "Invalid response from agent",
        )),
        Err(error @ JsonRpcRequestError::Nats(_)) => {
            Err(map_nats_error(error.into_nats_error(&subject_str)))
        }
    }
}
