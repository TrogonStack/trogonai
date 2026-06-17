//! Optional Session Kernel conversation sink.
//!
//! When the Session Kernel is enabled, the runner mirrors each turn's conversation
//! into the canonical event log in shadow mode (§3 event log, § Migration). This is
//! best-effort and strictly additive: the legacy `NatsSessionStore` remains the
//! authoritative store, and the default runner path (kernel disabled) is unchanged.

use async_trait::async_trait;
use buffa::{EnumValue, MessageField};
use bytes::Bytes;
use trogon_tools::{ContentBlock as ToolContentBlock, ImageSource, Message};
use trogonai_artifacts::{
    ArtifactEventContext, ArtifactStore, FetchLimits, ImageFetcher, StoreArtifactRequest, UrlImageOutcome,
    record_external_ref_image, resolve_url_image, store_and_emit_artifact_created,
};
use trogonai_session_contracts::__buffa::oneof::content_block::Kind as BlockKind;
use trogonai_session_contracts::__buffa::oneof::tool_call_result::Kind as ToolResultKind;
use trogonai_session_contracts::{
    Actor, ActorType, CanonicalMessage, CanonicalToolCall, ContentBlock, CorrelationId, EventId, IdempotencyKey,
    OperationId, SessionConfig, SessionId, TextToolResult, ToolCallResult, ToolCallStatus, ToolResultBlock,
    ToolUseBlock,
};
use trogonai_session_kernel::{
    EventLogBackend, SessionKernel, SessionKernelError, SessionLeaseFactory, SessionMutatingOperation,
    shadow_sync_from_conversation,
};

/// Build the FULL canonical tool-call view from the authoritative session messages
/// (`state.messages`) — full input JSON and full output, with NO summarization or
/// truncation. This is the no-lossy baseline the shadow divergence check compares
/// the (lossy, export-derived) materialized tool calls against (§ Migration "comparar
/// snapshot materializado contra estado actual"; § No-Lossy Contract).
pub fn full_tool_calls_from_messages(messages: &[Message]) -> Vec<CanonicalToolCall> {
    use std::collections::HashMap;

    let mut calls: Vec<CanonicalToolCall> = Vec::new();
    let mut results: HashMap<String, String> = HashMap::new();
    for message in messages {
        for block in &message.content {
            match block {
                ToolContentBlock::ToolUse { id, name, input, .. } => {
                    calls.push(CanonicalToolCall {
                        id: id.clone(),
                        tool_execution_id: id.clone(),
                        name: name.clone(),
                        input_json: input.to_string(),
                        ..CanonicalToolCall::default()
                    });
                }
                ToolContentBlock::ToolResult { tool_use_id, content } => {
                    results.insert(tool_use_id.clone(), content.clone());
                }
                _ => {}
            }
        }
    }
    for call in &mut calls {
        if let Some(output) = results.get(&call.id) {
            call.status = EnumValue::Known(ToolCallStatus::Completed);
            call.result = MessageField::some(ToolCallResult {
                kind: Some(
                    TextToolResult {
                        content: output.clone(),
                        ..TextToolResult::default()
                    }
                    .into(),
                ),
                ..ToolCallResult::default()
            });
        }
    }
    calls
}

/// Build the canonical conversation from the authoritative session messages with
/// FULL, structured content blocks — tool_use/tool_result blocks carry the complete
/// input JSON and output (no summarization or text-flattening). This keeps the
/// canonical transcript no-lossy (§11 "content blocks … input JSON completo, result
/// completo"; § No-Lossy Contract: summaries belong only in projections, not the
/// canonical truth).
pub fn full_canonical_messages_from_messages(messages: &[Message]) -> Vec<CanonicalMessage> {
    messages
        .iter()
        .enumerate()
        .map(|(idx, message)| {
            let content = message
                .content
                .iter()
                .map(|block| {
                    let kind = match block {
                        ToolContentBlock::Text { text } => BlockKind::Text(text.clone()),
                        ToolContentBlock::Thinking { thinking } => BlockKind::Thinking(thinking.clone()),
                        ToolContentBlock::ToolUse {
                            id,
                            name,
                            input,
                            parent_tool_use_id,
                        } => BlockKind::ToolUse(Box::new(ToolUseBlock {
                            id: id.clone(),
                            name: name.clone(),
                            input_json: input.to_string(),
                            parent_tool_use_id: parent_tool_use_id.clone(),
                            ..ToolUseBlock::default()
                        })),
                        ToolContentBlock::ToolResult { tool_use_id, content } => {
                            BlockKind::ToolResult(Box::new(ToolResultBlock {
                                tool_use_id: tool_use_id.clone(),
                                result: MessageField::some(ToolCallResult {
                                    kind: Some(
                                        TextToolResult {
                                            content: content.clone(),
                                            ..TextToolResult::default()
                                        }
                                        .into(),
                                    ),
                                    ..ToolCallResult::default()
                                }),
                                ..ToolResultBlock::default()
                            }))
                        }
                        ToolContentBlock::Image { .. } => BlockKind::Text("[image]".to_string()),
                    };
                    ContentBlock {
                        kind: Some(kind),
                        ..ContentBlock::default()
                    }
                })
                .collect();
            CanonicalMessage {
                message_id: format!("msg_{idx}"),
                role: message.role.clone(),
                content,
                ..CanonicalMessage::default()
            }
        })
        .collect()
}

/// The legacy degraded marker used when an image cannot be persisted as an artifact.
/// The shadow path is best-effort and non-fatal (§ Migration "shadow … best-effort"),
/// so a storage failure degrades to this marker rather than dropping the turn.
fn fallback_image_block() -> BlockKind {
    BlockKind::Text("[image]".to_string())
}

/// Persist an image content block as a canonical `image_ref` so the image bytes are
/// never lost (§ "Politica para imagenes URL y Base64", §966-1008). This replaces the
/// old flattening of images to a `"[image]"` text marker, honoring the No-Lossy
/// Contract (§ No-Lossy Contract: canonical truth must not drop content).
///
/// - **Base64**: decode → validate/sniff the real MIME → store the decoded bytes
///   (sha256 over the bytes, never the string) and reference them.
/// - **URL**: fetch (with the §979 limits) → validate → store the fetched bytes and
///   reference them; on a fetch failure, record a degraded `external_ref` (no bytes,
///   availability = ExternalRef) and keep the marker inline.
///
/// Best-effort: any storage failure falls back to [`fallback_image_block`].
pub async fn store_image_block<E, S, L, Os, F>(
    kernel: &SessionKernel<E, S, L>,
    store: &ArtifactStore<Os>,
    fetcher: &F,
    context: &ArtifactEventContext,
    session_id: &SessionId,
    source: &ImageSource,
    now: buffa_types::google::protobuf::Timestamp,
) -> BlockKind
where
    E: EventLogBackend,
    S: trogon_nats::jetstream::JetStreamKvGet
        + trogon_nats::jetstream::JetStreamKvEntry
        + trogon_nats::jetstream::JetStreamKvCreate
        + trogon_nats::jetstream::JetStreamKeyValueUpdate
        + Clone
        + Send
        + Sync
        + 'static,
    L: SessionLeaseFactory + Clone + Send + Sync + 'static,
    Os: trogon_nats::jetstream::ObjectStorePut
        + trogon_nats::jetstream::ObjectStoreGet
        + Clone
        + Send
        + Sync
        + 'static,
    F: ImageFetcher,
{
    // A fresh causing-event id per image keeps each artifact's metadata.event_id
    // distinct in the canonical log.
    let Ok(event_id) = EventId::new(format!("evt_img_{}", uuid::Uuid::new_v4())) else {
        return fallback_image_block();
    };

    let stored = match source {
        ImageSource::Base64 { media_type, data } => {
            // § Base64: decode and store the raw bytes (the sha256 is computed over the
            // decoded bytes by the store, never over the source string).
            let Ok(request) =
                StoreArtifactRequest::from_base64_image(session_id.clone(), event_id, media_type.clone(), data.as_str())
            else {
                return fallback_image_block();
            };
            store_and_emit_artifact_created(kernel, store, request, context.clone()).await
        }
        ImageSource::Url { url } => {
            // § URL: fetch → validate → store; on failure, degrade to an external_ref.
            match resolve_url_image(fetcher, session_id.clone(), event_id, url, now.clone(), &FetchLimits::default())
                .await
            {
                UrlImageOutcome::Stored(request) => {
                    store_and_emit_artifact_created(kernel, store, *request, context.clone()).await
                }
                UrlImageOutcome::Degraded { source_url, .. } => {
                    // Record the degraded reference (still an artifact_created in the
                    // log, with no bytes) so the Safety Gate can see it on a switch.
                    let _ = record_external_ref_image(kernel, context, session_id, &source_url, "image", now).await;
                    return fallback_image_block();
                }
            }
        }
    };

    match stored {
        Ok((stored, _)) => BlockKind::ImageRef(Box::new(stored.to_artifact_ref())),
        Err(_) => fallback_image_block(),
    }
}

/// The inline tool-result form: the FULL output kept as text in the event (used for
/// small outputs and as the best-effort fallback when storage fails).
fn inline_tool_result(output: &str) -> ToolCallResult {
    ToolCallResult {
        kind: Some(
            TextToolResult {
                content: output.to_string(),
                ..TextToolResult::default()
            }
            .into(),
        ),
        ..ToolCallResult::default()
    }
}

/// Build the canonical tool-call result, claim-checking a large output to an artifact
/// ref (§ "small output -> inline en evento; large output -> object store +
/// artifact_ref", §925-927; No-Lossy §2205: large content must move to artifact refs
/// with checksum and content length, never be lost). A small output (within
/// `inline_limit`) stays inline as text; a larger output is persisted as an artifact and
/// referenced by an `artifact_ref` carrying its sha256/size. Best-effort: any storage
/// failure falls back to inline text so the shadow path stays non-fatal.
pub async fn canonical_tool_result<E, S, L, Os>(
    kernel: &SessionKernel<E, S, L>,
    store: &ArtifactStore<Os>,
    context: &ArtifactEventContext,
    session_id: &SessionId,
    output: &str,
    inline_limit: usize,
) -> ToolCallResult
where
    E: EventLogBackend,
    S: trogon_nats::jetstream::JetStreamKvGet
        + trogon_nats::jetstream::JetStreamKvEntry
        + trogon_nats::jetstream::JetStreamKvCreate
        + trogon_nats::jetstream::JetStreamKeyValueUpdate
        + Clone
        + Send
        + Sync
        + 'static,
    L: SessionLeaseFactory + Clone + Send + Sync + 'static,
    Os: trogon_nats::jetstream::ObjectStorePut
        + trogon_nats::jetstream::ObjectStoreGet
        + Clone
        + Send
        + Sync
        + 'static,
{
    // § small output -> inline in the event.
    if output.len() <= inline_limit {
        return inline_tool_result(output);
    }
    // § large output -> object store + artifact_ref. A fresh causing-event id keeps each
    // artifact's metadata.event_id distinct in the canonical log.
    let Ok(event_id) = EventId::new(format!("evt_tool_{}", uuid::Uuid::new_v4())) else {
        return inline_tool_result(output);
    };
    let request = StoreArtifactRequest::new(session_id.clone(), event_id, "text/plain", Bytes::from(output.to_owned()));
    match store_and_emit_artifact_created(kernel, store, request, context.clone()).await {
        Ok((stored, _)) => ToolCallResult {
            kind: Some(ToolResultKind::ArtifactRef(Box::new(stored.to_artifact_ref()))),
            ..ToolCallResult::default()
        },
        Err(_) => inline_tool_result(output),
    }
}

fn shadow_actor() -> Actor {
    Actor {
        r#type: EnumValue::Known(ActorType::Kernel),
        id: "acp-runner-shadow".to_string(),
        ..Actor::default()
    }
}

/// A fresh artifact-event context for a shadow sync. The ids are structurally valid by
/// construction (prefix + uuid), so building them never fails in practice.
fn artifact_context() -> ArtifactEventContext {
    let suffix = uuid::Uuid::new_v4();
    ArtifactEventContext {
        operation_id: OperationId::new(format!("op_shadow_{suffix}")).expect("valid operation id"),
        correlation_id: CorrelationId::new(format!("corr_shadow_{suffix}")).expect("valid correlation id"),
        idempotency_key: IdempotencyKey::new(format!("idem_shadow_{suffix}")).expect("valid idempotency key"),
        causation_id: None,
        actor: shadow_actor(),
    }
}

/// Build the FULL canonical conversation AND tool calls from the authoritative messages,
/// store-aware: a large tool output is claim-checked to an artifact ref (§925-927) and an
/// image is persisted as an artifact ref (§966-1008) instead of being kept inline /
/// flattened to `"[image]"`. Each tool output is stored ONCE and its ref reused by both
/// the transcript block and the structured tool call (no double storage).
#[allow(clippy::too_many_arguments)]
async fn store_aware_conversation_and_tool_calls<E, S, L, Os, F>(
    kernel: &SessionKernel<E, S, L>,
    store: &ArtifactStore<Os>,
    fetcher: &F,
    context: &ArtifactEventContext,
    session_id: &SessionId,
    messages: &[Message],
    inline_limit: usize,
    now: buffa_types::google::protobuf::Timestamp,
) -> (Vec<CanonicalMessage>, Vec<CanonicalToolCall>)
where
    E: EventLogBackend,
    S: trogon_nats::jetstream::JetStreamKvGet
        + trogon_nats::jetstream::JetStreamKvEntry
        + trogon_nats::jetstream::JetStreamKvCreate
        + trogon_nats::jetstream::JetStreamKeyValueUpdate
        + Clone
        + Send
        + Sync
        + 'static,
    L: SessionLeaseFactory + Clone + Send + Sync + 'static,
    Os: trogon_nats::jetstream::ObjectStorePut
        + trogon_nats::jetstream::ObjectStoreGet
        + Clone
        + Send
        + Sync
        + 'static,
    F: ImageFetcher,
{
    use std::collections::HashMap;

    // 1. Claim-check each tool output ONCE; reuse the ref in both representations.
    let mut results: HashMap<String, ToolCallResult> = HashMap::new();
    for message in messages {
        for block in &message.content {
            if let ToolContentBlock::ToolResult { tool_use_id, content } = block {
                let result = canonical_tool_result(kernel, store, context, session_id, content, inline_limit).await;
                results.insert(tool_use_id.clone(), result);
            }
        }
    }

    // 2. Structured tool calls (full input JSON + the claim-checked result).
    let mut tool_calls: Vec<CanonicalToolCall> = Vec::new();
    for message in messages {
        for block in &message.content {
            if let ToolContentBlock::ToolUse { id, name, input, .. } = block {
                let mut call = CanonicalToolCall {
                    id: id.clone(),
                    tool_execution_id: id.clone(),
                    name: name.clone(),
                    input_json: input.to_string(),
                    ..CanonicalToolCall::default()
                };
                if let Some(result) = results.get(id) {
                    call.status = EnumValue::Known(ToolCallStatus::Completed);
                    call.result = MessageField::some(result.clone());
                }
                tool_calls.push(call);
            }
        }
    }

    // 3. Canonical transcript: structured blocks, tool results as refs, images persisted.
    let mut conversation: Vec<CanonicalMessage> = Vec::new();
    for (idx, message) in messages.iter().enumerate() {
        let mut content = Vec::new();
        for block in &message.content {
            let kind = match block {
                ToolContentBlock::Text { text } => BlockKind::Text(text.clone()),
                ToolContentBlock::Thinking { thinking } => BlockKind::Thinking(thinking.clone()),
                ToolContentBlock::ToolUse {
                    id,
                    name,
                    input,
                    parent_tool_use_id,
                } => BlockKind::ToolUse(Box::new(ToolUseBlock {
                    id: id.clone(),
                    name: name.clone(),
                    input_json: input.to_string(),
                    parent_tool_use_id: parent_tool_use_id.clone(),
                    ..ToolUseBlock::default()
                })),
                ToolContentBlock::ToolResult { tool_use_id, content: output } => {
                    let result = results
                        .get(tool_use_id)
                        .cloned()
                        .unwrap_or_else(|| inline_tool_result(output));
                    BlockKind::ToolResult(Box::new(ToolResultBlock {
                        tool_use_id: tool_use_id.clone(),
                        result: MessageField::some(result),
                        ..ToolResultBlock::default()
                    }))
                }
                ToolContentBlock::Image { source } => {
                    store_image_block(kernel, store, fetcher, context, session_id, source, now.clone()).await
                }
            };
            content.push(ContentBlock {
                kind: Some(kind),
                ..ContentBlock::default()
            });
        }
        conversation.push(CanonicalMessage {
            message_id: format!("msg_{idx}"),
            role: message.role.clone(),
            content,
            ..CanonicalMessage::default()
        });
    }

    (conversation, tool_calls)
}

/// Best-effort mirror of the session conversation into the canonical event log.
///
/// Implementations MUST be non-fatal: they log and swallow errors so the live
/// prompt path is never affected by kernel issues.
#[async_trait]
pub trait ConversationSink: Send + Sync {
    /// `messages` are the authoritative `state.messages`. The sink builds the FULL,
    /// no-lossy canonical view — structured content blocks, complete tool input/output,
    /// and (Fase 5) large outputs / images persisted as artifact refs — and records it
    /// into the canonical log, so nothing is summarized as canonical truth (§11
    /// Transcript no-lossy; § No-Lossy Contract: summaries belong only in projections).
    async fn sync(&self, session_id: &str, messages: &[Message], cwd: &str);
}

/// Kernel-backed sink: records each turn's authoritative messages into the canonical
/// event log (shadow mode, idempotent per turn). When `artifacts_enabled` (Fase 5,
/// `artifact_store_enabled`), large tool outputs and images are claim-checked to artifact
/// refs; otherwise the full content is kept inline (still no-lossy).
pub struct KernelConversationSink<E, S, L, Os, F> {
    kernel: SessionKernel<E, S, L>,
    store: ArtifactStore<Os>,
    fetcher: F,
    artifacts_enabled: bool,
    inline_limit: usize,
}

impl<E, S, L, Os, F> KernelConversationSink<E, S, L, Os, F> {
    pub fn new(
        kernel: SessionKernel<E, S, L>,
        store: ArtifactStore<Os>,
        fetcher: F,
        artifacts_enabled: bool,
        inline_limit: usize,
    ) -> Self {
        Self {
            kernel,
            store,
            fetcher,
            artifacts_enabled,
            inline_limit,
        }
    }
}

#[async_trait]
impl<E, S, L, Os, F> ConversationSink for KernelConversationSink<E, S, L, Os, F>
where
    E: EventLogBackend,
    S: trogon_nats::jetstream::JetStreamKvGet
        + trogon_nats::jetstream::JetStreamKvEntry
        + trogon_nats::jetstream::JetStreamKvCreate
        + trogon_nats::jetstream::JetStreamKeyValueUpdate
        + Clone
        + Send
        + Sync
        + 'static,
    L: SessionLeaseFactory + Clone + Send + Sync + 'static,
    Os: trogon_nats::jetstream::ObjectStorePut
        + trogon_nats::jetstream::ObjectStoreGet
        + Clone
        + Send
        + Sync
        + 'static,
    F: ImageFetcher + Send + Sync,
{
    async fn sync(&self, session_id: &str, messages: &[Message], cwd: &str) {
        let Ok(sid) = SessionId::new(session_id) else {
            return;
        };

        // The turn mirror is a mutating operation, so it follows the Session Lease
        // flow (acquire -> mutation -> append -> materialize -> release): it takes a
        // lease covering the whole logical operation, renewed by a heartbeat for its
        // duration, and releases it EXPLICITLY below. TTL is only the crash fallback.
        // Best-effort: if the session is busy, skip — the next sync is idempotent and
        // will catch up.
        let (guard, renewal) = match self
            .kernel
            .acquire_session_lease_renewing(&sid, SessionMutatingOperation::PromptTurn)
            .await
        {
            Ok(held) => held,
            Err(SessionKernelError::SessionBusy { .. }) => {
                tracing::debug!(
                    session_id,
                    "kernel conversation sink: session busy, skipping shadow sync"
                );
                return;
            }
            Err(err) => {
                tracing::warn!(session_id, error = %err, "kernel conversation sink: lease acquire failed");
                return;
            }
        };

        // Build the canonical view from the authoritative messages. With artifacts
        // enabled (Fase 5), large tool outputs are claim-checked and images persisted as
        // artifact refs; otherwise the full content is kept inline. Either way the
        // canonical truth is no-lossy (§11; § No-Lossy Contract) — never summarized.
        let (conversation, tool_calls) = if self.artifacts_enabled {
            store_aware_conversation_and_tool_calls(
                &self.kernel,
                &self.store,
                &self.fetcher,
                &artifact_context(),
                &sid,
                messages,
                self.inline_limit,
                buffa_types::google::protobuf::Timestamp::default(),
            )
            .await
        } else {
            (
                full_canonical_messages_from_messages(messages),
                full_tool_calls_from_messages(messages),
            )
        };

        // Record the FULL canonical conversation and tool calls into the log. No
        // divergence check is needed: the log equals state.messages by construction.
        // Errors are logged but never short-circuit the lease release.
        if let Err(err) =
            shadow_sync_from_conversation(&self.kernel, &sid, &conversation, cwd, None::<SessionConfig>).await
        {
            tracing::warn!(session_id, error = %err, "kernel conversation sink: shadow sync failed");
        } else if !tool_calls.is_empty()
            && let Err(err) = self
                .kernel
                .record_tool_calls(&sid, &tool_calls, shadow_actor(), Default::default())
                .await
        {
            tracing::warn!(session_id, error = %err, "kernel conversation sink: tool-call sync failed");
        }

        // Release the lease explicitly so the next turn's sync is not blocked until
        // TTL (Session Lease flow). Non-fatal: TTL still cleans up on failure.
        if let Err(err) = self.kernel.release_session_lease_renewing(guard, renewal).await {
            tracing::warn!(session_id, error = %err, "kernel conversation sink: lease release failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default, Clone)]
    struct RecordingSink {
        calls: Arc<Mutex<Vec<(String, usize)>>>,
    }

    #[async_trait]
    impl ConversationSink for RecordingSink {
        async fn sync(&self, session_id: &str, messages: &[Message], _cwd: &str) {
            self.calls
                .lock()
                .unwrap()
                .push((session_id.to_string(), messages.len()));
        }
    }

    #[tokio::test]
    async fn conversation_sink_is_invoked_with_messages() {
        let sink = RecordingSink::default();
        let messages = vec![Message {
            role: "user".to_string(),
            content: vec![],
        }];
        sink.sync("sess_1", &messages, "/repo").await;
        let calls = sink.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "sess_1");
        assert_eq!(calls[0].1, 1);
    }

    #[test]
    fn full_tool_calls_from_messages_preserves_full_input_and_output() {
        // A long input/output that the lossy portable export would summarize (240)
        // / truncate (500). The no-lossy producer must keep them in full.
        let long_input = format!("{{\"cmd\":\"{}\"}}", "x".repeat(400));
        let long_output = "y".repeat(900);
        let messages = vec![
            Message {
                role: "assistant".to_string(),
                content: vec![ToolContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({ "cmd": "x".repeat(400) }),
                    parent_tool_use_id: None,
                }],
            },
            Message {
                role: "user".to_string(),
                content: vec![ToolContentBlock::ToolResult {
                    tool_use_id: "t1".to_string(),
                    content: long_output.clone(),
                }],
            },
        ];

        let calls = full_tool_calls_from_messages(&messages);
        assert_eq!(calls.len(), 1);
        // Input JSON preserved in full (not truncated to 240).
        assert_eq!(calls[0].input_json, long_input);
        // Output preserved in full (not truncated to 500).
        let result = calls[0].result.as_option().expect("result present");
        match result.kind.as_ref().expect("result kind") {
            trogonai_session_contracts::__buffa::oneof::tool_call_result::Kind::Text(text) => {
                assert_eq!(text.content, long_output, "output must be preserved in full");
            }
            other => panic!("expected text result, got {other:?}"),
        }
    }

    #[test]
    fn full_canonical_messages_preserve_structured_tool_blocks_in_full() {
        // The canonical transcript must keep tool_use/tool_result as STRUCTURED
        // blocks with full input/output — not flattened to summarized text
        // (§11 "content blocks"; § No-Lossy Contract: no summaries in canonical).
        let long_cmd = "x".repeat(400);
        let long_output = "y".repeat(900);
        let messages = vec![
            Message {
                role: "assistant".to_string(),
                content: vec![ToolContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({ "cmd": long_cmd.clone() }),
                    parent_tool_use_id: None,
                }],
            },
            Message {
                role: "user".to_string(),
                content: vec![ToolContentBlock::ToolResult {
                    tool_use_id: "t1".to_string(),
                    content: long_output.clone(),
                }],
            },
        ];

        let canonical = full_canonical_messages_from_messages(&messages);
        assert_eq!(canonical.len(), 2);

        match canonical[0].content[0].kind.as_ref().expect("kind") {
            BlockKind::ToolUse(tu) => {
                assert_eq!(tu.id, "t1");
                assert_eq!(tu.name, "bash");
                assert!(tu.input_json.contains(&long_cmd), "full input JSON must be preserved");
            }
            other => panic!("expected structured tool_use block, got {other:?}"),
        }

        match canonical[1].content[0].kind.as_ref().expect("kind") {
            BlockKind::ToolResult(tr) => {
                let result = tr.result.as_option().expect("result present");
                match result.kind.as_ref().expect("result kind") {
                    trogonai_session_contracts::__buffa::oneof::tool_call_result::Kind::Text(text) => {
                        assert_eq!(text.content, long_output, "full output must be preserved");
                    }
                    other => panic!("expected text result, got {other:?}"),
                }
            }
            other => panic!("expected structured tool_result block, got {other:?}"),
        }
    }

    // --- store_image_block: images become canonical image_refs, never flattened ---

    use bytes::Bytes;
    use trogon_nats::jetstream::{MockJetStreamKvStore, MockObjectStore};
    use trogonai_artifacts::{ArtifactStoreConfig, FetchedImage, ImageFetchError};
    use trogonai_session_contracts::{CorrelationId, IdempotencyKey, OperationId, session_event_payload};
    use trogonai_session_kernel::{
        InMemoryEventLog, MockSessionLease, MockSessionLeaseFactory, SessionKernelConfig, SessionLeaseManager,
        SnapshotStore,
    };

    /// A 1x1 transparent PNG, base64-encoded (valid PNG magic bytes).
    const ONE_PX_PNG_B64: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

    struct OkPngFetcher;
    impl ImageFetcher for OkPngFetcher {
        async fn fetch(&self, _url: &str, _limits: &FetchLimits) -> Result<FetchedImage, ImageFetchError> {
            Ok(FetchedImage {
                content_type: Some("image/png".to_string()),
                bytes: Bytes::from_static(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3]),
            })
        }
    }

    struct FailingFetcher;
    impl ImageFetcher for FailingFetcher {
        async fn fetch(&self, _url: &str, _limits: &FetchLimits) -> Result<FetchedImage, ImageFetchError> {
            Err(ImageFetchError::BadStatus(404))
        }
    }

    fn test_kernel() -> (
        SessionKernel<InMemoryEventLog, MockJetStreamKvStore, MockSessionLeaseFactory>,
        InMemoryEventLog,
    ) {
        let snapshot_store = MockJetStreamKvStore::new();
        for _ in 0..16 {
            snapshot_store.enqueue_get_none();
        }
        let event_log = InMemoryEventLog::new();
        let kernel = SessionKernel::new(
            SessionKernelConfig::default(),
            event_log.clone(),
            SnapshotStore::new(snapshot_store, SessionKernelConfig::default()),
            SessionLeaseManager::new(MockSessionLeaseFactory::new(MockSessionLease::new()), "node-1"),
        );
        (kernel, event_log)
    }

    fn test_store() -> ArtifactStore<MockObjectStore> {
        ArtifactStore::new(
            MockObjectStore::new(),
            ArtifactStoreConfig::from_session_kernel(&SessionKernelConfig::default()),
        )
    }

    fn test_context() -> ArtifactEventContext {
        ArtifactEventContext {
            operation_id: OperationId::new("op_img").unwrap(),
            correlation_id: CorrelationId::new("corr_img").unwrap(),
            idempotency_key: IdempotencyKey::new("idem_img").unwrap(),
            causation_id: None,
            actor: shadow_actor(),
        }
    }

    fn ts() -> buffa_types::google::protobuf::Timestamp {
        buffa_types::google::protobuf::Timestamp {
            seconds: 1_700_000_000,
            nanos: 0,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn base64_image_is_stored_as_image_ref_not_flattened() {
        // § Base64: decode → store bytes → reference. The old path flattened this to a
        // "[image]" text marker, losing the image (violating the No-Lossy Contract).
        let (kernel, _log) = test_kernel();
        let store = test_store();
        let sid = SessionId::new("sess_img").unwrap();
        let source = ImageSource::Base64 {
            media_type: "image/png".to_string(),
            data: ONE_PX_PNG_B64.to_string(),
        };

        let block = store_image_block(&kernel, &store, &OkPngFetcher, &test_context(), &sid, &source, ts()).await;

        match block {
            BlockKind::ImageRef(_) => {}
            other => panic!("expected canonical image_ref, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn url_image_fetched_ok_is_stored_as_image_ref() {
        // § URL: fetch → validate → store bytes → reference.
        let (kernel, _log) = test_kernel();
        let store = test_store();
        let sid = SessionId::new("sess_img").unwrap();
        let source = ImageSource::Url {
            url: "https://host/img.png".to_string(),
        };

        let block = store_image_block(&kernel, &store, &OkPngFetcher, &test_context(), &sid, &source, ts()).await;

        assert!(matches!(block, BlockKind::ImageRef(_)));
    }

    #[tokio::test]
    async fn url_image_fetch_failure_degrades_to_external_ref_marker() {
        // § URL degraded: a fetch failure keeps a "[image]" marker inline but still
        // records a degraded external_ref artifact_created so the Safety Gate sees it.
        let (kernel, log) = test_kernel();
        let store = test_store();
        let sid = SessionId::new("sess_img").unwrap();
        let source = ImageSource::Url {
            url: "https://host/missing.png".to_string(),
        };

        let block = store_image_block(&kernel, &store, &FailingFetcher, &test_context(), &sid, &source, ts()).await;

        match block {
            BlockKind::Text(text) => assert_eq!(text, "[image]"),
            other => panic!("expected degraded text marker, got {other:?}"),
        }
        let events = log.read_session_events(&sid).await.unwrap();
        let has_artifact = events.iter().any(|event| {
            matches!(
                event.payload.as_option().and_then(|p| p.kind.as_ref()),
                Some(session_event_payload::Kind::ArtifactCreated(_))
            )
        });
        assert!(has_artifact, "expected a degraded artifact_created event in the log");
    }

    // --- canonical_tool_result: large tool outputs claim-check to artifact refs ---

    #[tokio::test]
    async fn small_tool_output_stays_inline() {
        // § "small output -> inline en evento".
        let (kernel, _log) = test_kernel();
        let store = test_store();
        let sid = SessionId::new("sess_tool").unwrap();

        let result = canonical_tool_result(&kernel, &store, &test_context(), &sid, "ok", 1024).await;

        match result.kind {
            Some(ToolResultKind::Text(text)) => assert_eq!(text.content, "ok"),
            other => panic!("expected inline text for a small output, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn large_tool_output_is_claim_checked_to_an_artifact_ref() {
        // § "large output -> object store + artifact_ref"; No-Lossy §2202: the ref must
        // carry checksum + byte size, and the output must NOT be flattened to inline
        // text as canonical truth.
        let (kernel, _log) = test_kernel();
        let store = test_store();
        let sid = SessionId::new("sess_tool").unwrap();
        let big = "y".repeat(5000);

        let result = canonical_tool_result(&kernel, &store, &test_context(), &sid, &big, 1024).await;

        match result.kind {
            Some(ToolResultKind::ArtifactRef(art_ref)) => {
                assert_eq!(art_ref.size_bytes, 5000, "the artifact ref must carry the full byte size");
                assert!(
                    !art_ref.sha256.is_empty(),
                    "the artifact ref must carry the checksum (No-Lossy §2202)"
                );
            }
            other => panic!("expected a claim-checked artifact ref for a large output, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn store_aware_conversion_claim_checks_outputs_and_persists_images() {
        // The wired producer: a large tool output becomes an artifact ref in BOTH the
        // structured tool call and the transcript block, and an image becomes an
        // image_ref instead of being flattened to "[image]".
        let (kernel, _log) = test_kernel();
        let store = test_store();
        let sid = SessionId::new("sess_conv").unwrap();
        let big = "z".repeat(5000);
        let messages = vec![
            Message {
                role: "assistant".to_string(),
                content: vec![
                    ToolContentBlock::ToolUse {
                        id: "t1".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({ "cmd": "ls" }),
                        parent_tool_use_id: None,
                    },
                    ToolContentBlock::Image {
                        source: ImageSource::Base64 {
                            media_type: "image/png".to_string(),
                            data: ONE_PX_PNG_B64.to_string(),
                        },
                    },
                ],
            },
            Message {
                role: "user".to_string(),
                content: vec![ToolContentBlock::ToolResult {
                    tool_use_id: "t1".to_string(),
                    content: big.clone(),
                }],
            },
        ];

        let (conversation, tool_calls) = store_aware_conversation_and_tool_calls(
            &kernel,
            &store,
            &OkPngFetcher,
            &test_context(),
            &sid,
            &messages,
            1024,
            ts(),
        )
        .await;

        // Large tool output -> claim-checked artifact ref in the structured tool call.
        let call = tool_calls.iter().find(|c| c.id == "t1").expect("tool call present");
        let result = call.result.as_option().expect("tool result present");
        assert!(
            matches!(result.kind.as_ref(), Some(ToolResultKind::ArtifactRef(_))),
            "large tool output must be a claim-checked artifact ref, not inline text"
        );

        // ...and the same ref in the transcript's tool_result block.
        let user_msg = &conversation[1];
        let tool_block_is_ref = user_msg.content.iter().any(|b| match b.kind.as_ref() {
            Some(BlockKind::ToolResult(tr)) => {
                matches!(tr.result.as_option().and_then(|r| r.kind.as_ref()), Some(ToolResultKind::ArtifactRef(_)))
            }
            _ => false,
        });
        assert!(tool_block_is_ref, "transcript tool_result must also reference the artifact");

        // Image -> image_ref, not flattened to "[image]".
        let assistant = &conversation[0];
        let has_image_ref = assistant
            .content
            .iter()
            .any(|b| matches!(b.kind.as_ref(), Some(BlockKind::ImageRef(_))));
        assert!(has_image_ref, "image must be persisted as an image_ref, not flattened");
    }
}
