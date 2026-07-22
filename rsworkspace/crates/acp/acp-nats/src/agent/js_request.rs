use agent_client_protocol::{Error, ErrorCode};
use async_nats::jetstream::AckKind;
use futures::StreamExt;
use jsonrpc_nats::RequestId;
use serde::de::DeserializeOwned;
use std::time::Duration;
use tokio::time::timeout;
use trogon_nats::REQ_ID_HEADER;
use trogon_nats::jetstream::{
    JetStreamConsumer as _, JetStreamCreateConsumer as _, JetStreamGetStream, JetStreamPublisher, JsAck as _,
    JsAckWith as _, JsMessageRef as _, JsRequestMessage,
};

use crate::acp_prefix::AcpPrefix;
use crate::constants::SESSION_ID_HEADER;
use crate::jetstream::{consumers, streams};
use crate::req_id::ReqId;
use crate::wire::{WireError, decode_response, encode_request};

#[allow(clippy::too_many_arguments)]
pub async fn js_request<J, Req, Res>(
    js: &J,
    subject: &str,
    method: &str,
    request: &Req,
    prefix: &AcpPrefix,
    session_id: &crate::session_id::AcpSessionId,
    req_id: &ReqId,
    operation_timeout: Duration,
) -> agent_client_protocol::Result<Res>
where
    J: JetStreamPublisher + JetStreamGetStream,
    trogon_nats::jetstream::JsMessageOf<J>: JsRequestMessage,
    Req: serde::Serialize,
    Res: DeserializeOwned,
{
    let responses_stream = streams::responses_stream_name(prefix);
    let resp_config = consumers::response_consumer(prefix, session_id, req_id);
    let stream = js
        .get_stream(&responses_stream)
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("get stream: {e}")))?;
    let resp_consumer = stream.create_consumer(resp_config).await.map_err(|e| {
        Error::new(
            ErrorCode::InternalError.into(),
            format!("create response consumer: {e}"),
        )
    })?;
    let mut resp_messages = resp_consumer
        .messages()
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("response messages: {e}")))?;

    let encoded = encode_request(method, RequestId::String(req_id.as_str().to_string()), request)
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("encode request: {e}")))?;

    let mut headers = encoded.headers;
    headers.insert(REQ_ID_HEADER, req_id.as_str());
    headers.insert(SESSION_ID_HEADER, session_id.as_str());

    js.publish_with_headers(subject.to_string(), headers, encoded.body)
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("js publish: {e}")))?
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("js ack: {e}")))?;

    match timeout(operation_timeout, resp_messages.next()).await {
        Ok(Some(Ok(js_msg))) => {
            let message = js_msg.message();
            let response_headers = message.headers.clone().unwrap_or_default();
            match decode_response::<Res>(&response_headers, message.payload.as_ref()) {
                Ok(Ok(response)) => {
                    let _ = js_msg.ack().await;
                    Ok(response)
                }
                Ok(Err(agent_err)) => {
                    let _ = js_msg.ack().await;
                    Err(agent_err)
                }
                Err(WireError::Codec(_) | WireError::Deserialize(_) | WireError::UnexpectedMessage) => {
                    let _ = js_msg.ack_with(AckKind::Term).await;
                    Err(Error::new(ErrorCode::InternalError.into(), "bad response payload"))
                }
                Err(e) => {
                    let _ = js_msg.ack_with(AckKind::Term).await;
                    Err(Error::new(
                        ErrorCode::InternalError.into(),
                        format!("decode response: {e}"),
                    ))
                }
            }
        }
        Ok(Some(Err(e))) => Err(Error::new(
            ErrorCode::InternalError.into(),
            format!("response consumer: {e}"),
        )),
        Ok(None) => Err(Error::new(
            ErrorCode::InternalError.into(),
            "response stream closed unexpectedly",
        )),
        Err(_elapsed) => Err(Error::new(
            ErrorCode::Other(crate::constants::AGENT_UNAVAILABLE).into(),
            "Request timed out; agent may be overloaded or unavailable",
        )),
    }
}

#[cfg(test)]
mod tests;
