use agent_client_protocol::{Error, ErrorCode, PromptRequest, PromptResponse, SessionNotification, StopReason};
use async_nats::jetstream::AckKind;
use futures::StreamExt;
use jsonrpc_nats::RequestId;
use tokio::time::timeout;
use tracing::{instrument, warn};
use trogon_nats::jetstream::{
    JetStreamConsumer as _, JetStreamCreateConsumer as _, JetStreamGetStream, JetStreamPublisher, JsAck as _,
    JsAckWith as _, JsMessageRef as _, JsRequestMessage,
};
use trogon_semconv::span::ACP_SESSION_PROMPT;

use crate::agent::Bridge;
use crate::constants::SESSION_ID_HEADER;
use crate::jetstream::{consumers, streams};
use crate::nats::parsing::SessionAgentMethod;
use crate::nats::{FlushClient, PublishClient, RequestClient, SubscribeClient, commands, responses};
use crate::req_id::ReqId;
use crate::session_id::AcpSessionId;
use crate::wire::{WireError, decode_notification_params, decode_response, encode_request};

pub use trogon_nats::REQ_ID_HEADER;

#[instrument(
    name = ACP_SESSION_PROMPT,
    skip(bridge, args),
    fields(session_id = %args.session_id)
)]
pub async fn handle<N, C, J>(
    bridge: &Bridge<N, C, J>,
    args: PromptRequest,
) -> agent_client_protocol::Result<PromptResponse>
where
    N: RequestClient + PublishClient + SubscribeClient + FlushClient,
    C: trogon_std::time::GetElapsed,
    J: JetStreamPublisher + JetStreamGetStream,
    trogon_nats::jetstream::JsMessageOf<J>: JsRequestMessage,
{
    let start = bridge.clock.now();

    let session_id = AcpSessionId::try_from(&args.session_id).map_err(|_| {
        bridge.metrics.record_error("prompt", "invalid_session_id");
        Error::new(ErrorCode::InvalidParams.into(), "Invalid session ID")
    })?;

    let req_id = ReqId::new();
    let prefix = bridge.config.acp_prefix_ref();

    let result = handle_js(bridge, bridge.js(), &args, &session_id, prefix, &req_id).await;

    bridge
        .metrics
        .record_request("prompt", bridge.clock.elapsed(start).as_secs_f64(), result.is_ok());

    result
}

async fn handle_js<N, C, J>(
    bridge: &Bridge<N, C, J>,
    js: &J,
    args: &PromptRequest,
    session_id: &AcpSessionId,
    prefix: &crate::acp_prefix::AcpPrefix,
    req_id: &ReqId,
) -> agent_client_protocol::Result<PromptResponse>
where
    N: SubscribeClient,
    C: trogon_std::time::GetElapsed,
    J: JetStreamPublisher + JetStreamGetStream,
    trogon_nats::jetstream::JsMessageOf<J>: JsRequestMessage,
{
    // Create consumers BEFORE publishing — same principle as subscribe-before-publish.
    // JetStream consumers with DeliverAll replay from stream start, so they'll see the
    // response even if the runner responds before we start consuming.
    let notifications_stream = streams::notifications_stream_name(prefix);
    let notif_config = consumers::prompt_notifications_consumer(prefix, session_id, req_id);
    let notif_stream = js.get_stream(&notifications_stream).await.map_err(|e| {
        Error::new(
            ErrorCode::InternalError.into(),
            format!("get notifications stream: {e}"),
        )
    })?;
    let notif_consumer = notif_stream.create_consumer(notif_config).await.map_err(|e| {
        Error::new(
            ErrorCode::InternalError.into(),
            format!("create notification consumer: {e}"),
        )
    })?;
    let mut notif_messages = notif_consumer
        .messages()
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("notification messages: {e}")))?;

    let responses_stream = streams::responses_stream_name(prefix);
    let resp_config = consumers::prompt_response_consumer(prefix, session_id, req_id);
    let resp_stream = js
        .get_stream(&responses_stream)
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("get responses stream: {e}")))?;
    let resp_consumer = resp_stream.create_consumer(resp_config).await.map_err(|e| {
        Error::new(
            ErrorCode::InternalError.into(),
            format!("create response consumer: {e}"),
        )
    })?;
    let mut resp_messages = resp_consumer
        .messages()
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("response messages: {e}")))?;

    // Cancel still uses core NATS — it's a fire-and-forget signal, not persisted.
    let mut cancel_sub = bridge
        .nats
        .subscribe(responses::CancelledSubject::new(prefix, session_id))
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("subscribe cancelled: {e}")))?;

    // Now publish — consumers are ready, no race condition.
    let encoded = encode_request(
        SessionAgentMethod::Prompt.wire_method(),
        RequestId::String(req_id.as_str().to_string()),
        args,
    )
    .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("encode request: {e}")))?;

    let mut headers = encoded.headers;
    headers.insert(REQ_ID_HEADER, req_id.as_str());
    headers.insert(SESSION_ID_HEADER, session_id.as_str());

    let prompt_subject = commands::PromptSubject::new(prefix, session_id);
    js.publish_with_headers(prompt_subject, headers, encoded.body)
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("js publish: {e}")))?
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("js ack: {e}")))?;

    let op_timeout = bridge.config.prompt_timeout();

    loop {
        tokio::select! {
            notif = notif_messages.next() => {
                match notif {
                    None => {
                        bridge.metrics.record_error("prompt", "notification_stream_closed");
                        break Err(Error::new(
                            ErrorCode::InternalError.into(),
                            "notification stream closed unexpectedly",
                        ));
                    }
                    Some(Err(e)) => {
                        bridge.metrics.record_error("prompt", "notification_consumer_error");
                        break Err(Error::new(
                            ErrorCode::InternalError.into(),
                            format!("notification consumer: {e}"),
                        ));
                    }
                    Some(Ok(js_msg)) => {
                        let message = js_msg.message();
                        let notification_headers = message.headers.clone().unwrap_or_default();
                        let notification: SessionNotification = match decode_notification_params(
                            "session/update",
                            &notification_headers,
                            message.payload.as_ref(),
                        ) {
                            Ok(n) => n,
                            Err(e) => {
                                warn!(error = %e, "bad notification payload; skipping");
                                let _ = js_msg.ack().await;
                                continue;
                            }
                        };
                        let _ = js_msg.ack().await;
                        let _ = bridge.notification_sender.send(notification).await.inspect_err(|_| {
                            warn!("notification receiver dropped; continuing prompt");
                        });
                    }
                }
            }
            resp = timeout(op_timeout, resp_messages.next()) => {
                match resp {
                    Ok(Some(Ok(js_msg))) => {
                        let message = js_msg.message();
                        let response_headers = message.headers.clone().unwrap_or_default();
                        match decode_response::<PromptResponse>(&response_headers, message.payload.as_ref()) {
                            Ok(Ok(response)) => {
                                let _ = js_msg.ack().await;
                                break Ok(response);
                            }
                            Ok(Err(agent_err)) => {
                                let _ = js_msg.ack().await;
                                break Err(agent_err);
                            }
                            Err(WireError::Codec(_) | WireError::Deserialize(_) | WireError::UnexpectedMessage) => {
                                let _ = js_msg.ack_with(AckKind::Term).await;
                                bridge.metrics.record_error("prompt", "bad_response_payload");
                                break Err(Error::new(
                                    ErrorCode::InternalError.into(),
                                    "bad response payload",
                                ));
                            }
                            Err(e) => {
                                let _ = js_msg.ack_with(AckKind::Term).await;
                                bridge.metrics.record_error("prompt", "bad_response_payload");
                                break Err(Error::new(
                                    ErrorCode::InternalError.into(),
                                    format!("decode response: {e}"),
                                ));
                            }
                        }
                    }
                    Ok(Some(Err(e))) => {
                        bridge.metrics.record_error("prompt", "response_consumer_error");
                        break Err(Error::new(
                            ErrorCode::InternalError.into(),
                            format!("response consumer: {e}"),
                        ));
                    }
                    Ok(None) => {
                        bridge.metrics.record_error("prompt", "response_stream_closed");
                        break Err(Error::new(
                            ErrorCode::InternalError.into(),
                            "response stream closed unexpectedly",
                        ));
                    }
                    Err(_elapsed) => {
                        bridge.metrics.record_error("prompt", "prompt_timeout");
                        break Err(Error::new(
                            ErrorCode::InternalError.into(),
                            "prompt timed out waiting for runner",
                        ));
                    }
                }
            }
            _ = cancel_sub.next() => {
                break Ok(PromptResponse::new(StopReason::Cancelled));
            }
        }
    }
}

#[cfg(test)]
mod tests;
