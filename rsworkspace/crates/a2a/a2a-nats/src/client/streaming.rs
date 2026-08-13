use std::sync::{Arc, Mutex};

use a2a::types::SendMessageResponse;
use jsonrpc_nats::RequestId;
use serde::Serialize;
use tokio::time::timeout;
use trogon_nats::RequestClient;
use trogon_nats::jetstream::{JetStreamCreateConsumer, JetStreamGetStream, JsAck, JsMessageOf, JsMessageRef};

use a2a_identity_types::MintedUserJwt;

use crate::a2a_prefix::A2aPrefix;
use crate::jetstream::consumers::stream_events_consumer;
use crate::jetstream::streams::events_stream_name;
use crate::req_id::ReqId;

use crate::task_id::A2aTaskId;

use super::error::ClientError;
use super::event_stream::{TypedEventStream, build_event_stream, empty_event_stream};
use super::gateway_headers::{agent_rpc_headers, gateway_ingress_rpc_headers};
use super::validated::ValidatedRpc;
use super::wire::{decode_client_response, encode_client_request, merge_jsonrpc_headers};

pub struct StreamingRequest<'a, N, J> {
    pub nats: &'a N,
    pub js: &'a J,
    pub subject: &'a str,
    pub method: &'static str,
    pub req_id: &'a ReqId,
    pub prefix: &'a A2aPrefix,
    pub op_timeout: std::time::Duration,
    pub gateway_caller_jwt: Option<&'a MintedUserJwt>,
}

pub async fn send_streaming<N, J, Req>(
    ctx: StreamingRequest<'_, N, J>,
    params: &Req,
) -> Result<(SendMessageResponse, TypedEventStream), ClientError>
where
    N: RequestClient,
    J: JetStreamGetStream,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
    Req: Serialize,
{
    let (validated, stream) = send_streaming_validated(ctx, params).await?;
    Ok((validated.value, stream))
}

pub async fn send_streaming_validated<N, J, Req>(
    ctx: StreamingRequest<'_, N, J>,
    params: &Req,
) -> Result<(ValidatedRpc<SendMessageResponse>, TypedEventStream), ClientError>
where
    N: RequestClient,
    J: JetStreamGetStream,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
    Req: Serialize,
{
    let StreamingRequest {
        nats,
        js,
        subject,
        method,
        req_id,
        prefix,
        op_timeout,
        gateway_caller_jwt,
    } = ctx;
    let encoded = encode_client_request(method, RequestId::String(req_id.as_str().to_owned()), params)
        .map_err(|e| ClientError::Serialize(<serde_json::Error as serde::de::Error>::custom(format!("{e}"))))?;

    let headers = match gateway_caller_jwt {
        Some(jwt) => gateway_ingress_rpc_headers(req_id, jwt)?,
        None => agent_rpc_headers(req_id),
    };
    let headers = merge_jsonrpc_headers(headers, encoded.headers);

    let msg = timeout(
        op_timeout,
        nats.request_with_headers(subject.to_string(), headers, encoded.body),
    )
    .await
    .map_err(|_| ClientError::Timeout {
        subject: subject.to_string(),
    })?
    .map_err(|e| ClientError::Transport(e.to_string()))?;

    let response_headers = msg.headers.unwrap_or_default();
    let body = msg.payload.clone();
    let result = match decode_client_response::<SendMessageResponse>(&response_headers, &body)
        .map_err(|e| ClientError::Deserialize(<serde_json::Error as serde::de::Error>::custom(format!("{e}"))))?
    {
        Ok(result) => result,
        Err((code, message)) => return Err(ClientError::from_jsonrpc_code(code, message)),
    };

    // The consumer is opened after the reply, not before it: task event subjects
    // are scoped to the task (ADR#0055), and the bootstrap reply is where the task
    // id comes from. Nothing is missed in the gap, because `A2A_EVENTS` retains by
    // limits and the consumer delivers from sequence 0. The subject no longer names
    // the request, so the pump filters on `Trogon-Req-Id` to keep another caller's
    // subscription to the same task out of this stream.
    let event_stream = match &result {
        SendMessageResponse::Task(task) => {
            let task_id = A2aTaskId::new(task.id.clone())
                .map_err(|e| ClientError::ConsumerSetup(format!("agent returned an invalid task id: {e}")))?;
            open_task_stream(js, prefix, &task_id, req_id).await?
        }
        SendMessageResponse::Message(_) => empty_event_stream(),
    };

    Ok((ValidatedRpc::new(result, body), event_stream))
}

pub async fn open_task_stream<J>(
    js: &J,
    prefix: &A2aPrefix,
    task_id: &A2aTaskId,
    req_id: &ReqId,
) -> Result<TypedEventStream, ClientError>
where
    J: JetStreamGetStream,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
{
    let stream_name = events_stream_name(prefix);
    let stream = js
        .get_stream(&stream_name)
        .await
        .map_err(|e| ClientError::ConsumerSetup(format!("get stream '{stream_name}': {e}")))?;

    let consumer_config = stream_events_consumer(prefix, task_id);

    let last_seq = Arc::new(Mutex::new(0u64));

    let consumer = stream
        .create_consumer(consumer_config)
        .await
        .map_err(|e| ClientError::ConsumerSetup(format!("create consumer: {e}")))?;

    Ok(build_event_stream(consumer, last_seq, Some(req_id.clone())))
}

#[cfg(test)]
mod tests;
