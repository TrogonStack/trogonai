use agent_client_protocol::{Error, ErrorCode};
use async_nats::jetstream::AckKind;
use bytes::Bytes;
use futures::StreamExt;
use serde::de::DeserializeOwned;
use std::time::Duration;
use tokio::time::timeout;
use trogon_nats::REQ_ID_HEADER;
use trogon_nats::jetstream::{
    JetStreamConsumer as _, JetStreamCreateConsumer as _, JetStreamGetStream, JetStreamPublisher, JsAck as _,
    JsAckWith as _, JsMessageRef as _, JsRequestMessage,
};
use trogon_std::JsonSerialize;

use crate::acp_prefix::AcpPrefix;
use crate::constants::SESSION_ID_HEADER;
use crate::jetstream::{consumers, streams};
use crate::req_id::ReqId;

#[allow(clippy::too_many_arguments)]
pub async fn js_request<J, Req, Res, S>(
    js: &J,
    subject: &str,
    request: &Req,
    serializer: &S,
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
    S: JsonSerialize,
{
    // Create consumer BEFORE publishing — prevents missing the response if the
    // runner responds before we start consuming. DeliverAll replays from stream start.
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

    let payload_bytes = serializer
        .to_vec(request)
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("serialize: {e}")))?;

    let mut headers = async_nats::HeaderMap::new();
    headers.insert(REQ_ID_HEADER, req_id.as_str());
    headers.insert(SESSION_ID_HEADER, session_id.as_str());

    js.publish_with_headers(subject.to_string(), headers, Bytes::from(payload_bytes))
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("js publish: {e}")))?
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("js ack: {e}")))?;

    match timeout(operation_timeout, resp_messages.next()).await {
        Ok(Some(Ok(js_msg))) => match serde_json::from_slice::<Res>(js_msg.message().payload.as_ref()) {
            Ok(response) => {
                let _ = js_msg.ack().await;
                Ok(response)
            }
            Err(_) => {
                if let Ok(agent_err) = serde_json::from_slice::<Error>(js_msg.message().payload.as_ref()) {
                    let _ = js_msg.ack().await;
                    Err(agent_err)
                } else {
                    let _ = js_msg.ack_with(AckKind::Term).await;
                    Err(Error::new(ErrorCode::InternalError.into(), "bad response payload"))
                }
            }
        },
        Ok(Some(Err(e))) => Err(Error::new(
            ErrorCode::InternalError.into(),
            format!("response consumer: {e}"),
        )),
        Ok(None) => Err(Error::new(
            ErrorCode::InternalError.into(),
            "response stream closed unexpectedly",
        )),
        Err(_elapsed) => Err(Error::new(
            ErrorCode::InternalError.into(),
            "request timed out waiting for runner",
        )),
    }
}

#[cfg(test)]
mod tests;
