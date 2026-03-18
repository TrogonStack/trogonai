use agent_client_protocol::{
    ContentBlock, ContentChunk, Error, ErrorCode, PromptRequest, PromptResponse, SessionNotification,
    SessionUpdate, StopReason, TextContent, ToolCall, ToolCallStatus, ToolCallUpdate,
    ToolCallUpdateFields, UsageUpdate,
};
use bytes::Bytes;
use futures_util::StreamExt;
use tokio::time::timeout;
use tracing::warn;

use crate::agent::Bridge;
use crate::nats::{FlushClient, PublishClient, RequestClient, SubscribeClient, agent};
use crate::prompt_event::{PromptEvent, PromptPayload};
use crate::session_id::AcpSessionId;

pub async fn handle<N, C>(
    bridge: &Bridge<N, C>,
    args: PromptRequest,
) -> agent_client_protocol::Result<PromptResponse>
where
    N: RequestClient + PublishClient + SubscribeClient + FlushClient,
    C: trogon_std::time::GetElapsed,
{
    // 1. Validate session ID — reject before touching NATS
    let session_id = AcpSessionId::try_from(&args.session_id).map_err(|_| {
        Error::new(ErrorCode::InvalidParams.into(), "invalid session id")
    })?;

    // 2. Extract user message text from content blocks
    let user_message = args
        .prompt
        .iter()
        .filter_map(|block| {
            if let ContentBlock::Text(t) = block {
                Some(t.text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    // 3. Generate a unique request ID to correlate the response event stream
    let req_id = uuid::Uuid::new_v4().to_string();

    // 4. Subscribe to events BEFORE publishing the prompt — prevents losing the
    //    first event in case the runner is already running and responds instantly
    let events_subject =
        agent::prompt_events(bridge.config.acp_prefix(), session_id.as_ref(), &req_id);

    let mut subscriber = bridge.nats.subscribe(events_subject).await.map_err(|e| {
        Error::new(ErrorCode::InternalError.into(), format!("subscribe: {e}"))
    })?;

    // 5. Build and publish the prompt payload via NATS Core
    let payload = PromptPayload {
        req_id,
        session_id: session_id.to_string(),
        user_message,
    };
    let payload_bytes =
        serde_json::to_vec(&payload).map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))?;

    let prompt_subject = agent::prompt(bridge.config.acp_prefix(), session_id.as_ref());
    bridge
        .nats
        .publish_with_headers(
            prompt_subject,
            async_nats::HeaderMap::new(),
            Bytes::from(payload_bytes),
        )
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("publish: {e}")))?;

    bridge
        .nats
        .flush()
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("flush: {e}")))?;

    // 6. Stream events back to the ACP client until the runner signals Done
    // Prompts may be queued behind other running prompts — use a long timeout
    // so queued prompts don't time out while waiting for the runner.
    let op_timeout = std::time::Duration::from_secs(600); // 10 minutes

    loop {
        let msg = match timeout(op_timeout, subscriber.next()).await {
            Ok(Some(msg)) => msg,
            Ok(None) => {
                return Err(Error::new(
                    ErrorCode::InternalError.into(),
                    "prompt event stream closed unexpectedly",
                ));
            }
            Err(_elapsed) => {
                return Err(Error::new(
                    ErrorCode::InternalError.into(),
                    "prompt timed out waiting for runner",
                ));
            }
        };

        let event: PromptEvent = match serde_json::from_slice(&msg.payload) {
            Ok(e) => e,
            Err(e) => {
                return Err(Error::new(
                    ErrorCode::InternalError.into(),
                    format!("bad event payload: {e}"),
                ));
            }
        };

        match event {
            PromptEvent::TextDelta { text } => {
                let notification = SessionNotification::new(
                    args.session_id.clone(),
                    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                        TextContent::new(text),
                    ))),
                );
                // Best-effort: if the receiver was dropped the turn still completes
                if bridge.notification_sender.send(notification).await.is_err() {
                    warn!("notification receiver dropped; continuing prompt");
                }
            }
            PromptEvent::Done { stop_reason } => {
                let sr = match stop_reason.as_str() {
                    "end_turn" => StopReason::EndTurn,
                    "max_tokens" => StopReason::MaxTokens,
                    "cancelled" => StopReason::Cancelled,
                    other => {
                        warn!(stop_reason = other, "unknown stop reason — using EndTurn");
                        StopReason::EndTurn
                    }
                };
                return Ok(PromptResponse::new(sr));
            }
            PromptEvent::Error { message } => {
                return Err(Error::new(ErrorCode::InternalError.into(), message));
            }
            PromptEvent::ToolCallStarted { id, name, input } => {
                let tool_call = ToolCall::new(id, name)
                    .status(ToolCallStatus::InProgress)
                    .raw_input(input);
                let notification = SessionNotification::new(
                    args.session_id.clone(),
                    SessionUpdate::ToolCall(tool_call),
                );
                if bridge.notification_sender.send(notification).await.is_err() {
                    warn!("notification receiver dropped; continuing prompt");
                }
            }
            PromptEvent::ToolCallFinished { id, output } => {
                let fields = ToolCallUpdateFields::new()
                    .status(ToolCallStatus::Completed)
                    .raw_output(serde_json::Value::String(output));
                let update = ToolCallUpdate::new(id, fields);
                let notification = SessionNotification::new(
                    args.session_id.clone(),
                    SessionUpdate::ToolCallUpdate(update),
                );
                if bridge.notification_sender.send(notification).await.is_err() {
                    warn!("notification receiver dropped; continuing prompt");
                }
            }
            PromptEvent::UsageUpdate { input_tokens, output_tokens } => {
                let used = (input_tokens as u64) + (output_tokens as u64);
                let size = 200_000u64; // Claude's typical context window
                let update = UsageUpdate::new(used, size);
                let notification = SessionNotification::new(
                    args.session_id.clone(),
                    SessionUpdate::UsageUpdate(update),
                );
                if bridge.notification_sender.send(notification).await.is_err() {
                    warn!("notification receiver dropped; continuing prompt");
                }
            }
        }
    }
}
