//! Fase 4 (§1875 "runners como bindings/adapters") shadow emission for the Codex runner.
//!
//! The session belongs to Trogonai, not the runner (§ Principio central). Each completed
//! turn's conversation is mirrored into the canonical Session Kernel event log + snapshot
//! (§1671 shadow mode: "emitir eventos en paralelo"; §3 event log; §11 / § No-Lossy
//! Contract: full role + text content preserved as canonical truth). The Codex runner
//! converts ITS OWN in-memory `PortableMessage` history into canonical messages — it does
//! not share a sink with other runners.
//!
//! Gated by the kernel feature flags (off by default) and strictly best-effort: a kernel
//! failure is logged and never blocks or breaks the prompt path.

use std::sync::Arc;

use async_trait::async_trait;
use buffa::EnumValue;
use buffa_types::google::protobuf::Timestamp;
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_runner_tools::portable_session::{PortableBlock, PortableMessage};
use trogonai_session_contracts::{
    Actor, ActorType, CanonicalMessage, ContentBlock, SessionId,
    __buffa::oneof::content_block::Kind as BlockKind,
};
use trogonai_session_kernel::{
    EventLog, EventLogBackend, SessionKernel, SessionKernelConfig, SessionKernelFeatureFlags,
    SessionKernelOperationalPolicy, SessionKvLeaseFactory, SessionLeaseFactory, SessionLeaseManager, SnapshotStore,
    provision_lease_store, provision_snapshot_store,
};

/// Records a completed turn's conversation into the canonical kernel (shadow mode).
#[async_trait]
pub trait ShadowRecorder: Send + Sync {
    async fn record_turn(&self, session_id: &str, history: &[PortableMessage], model: &str);
}

/// Render a Codex `PortableBlock` to canonical text, preserving the tool name and the
/// summary Codex keeps at this layer (§11 / §2192 no-lossy: the tool call name and the
/// available input/output must not be dropped as canonical truth — Codex only retains
/// summaries here, full IO lives in the subprocess).
fn render_block(block: &PortableBlock) -> String {
    match block {
        PortableBlock::Text { text } => text.clone(),
        PortableBlock::Thinking { text } => format!("[thinking] {text}"),
        PortableBlock::ToolUse { name, input_summary, .. } => format!("[tool: {name}] {input_summary}"),
        PortableBlock::ToolResult { output_summary, .. } => format!("[tool result] {output_summary}"),
    }
}

/// Convert the Codex runner's own in-memory history into canonical messages (§11 no-lossy:
/// role + content preserved). When a message carries structured blocks (tool calls/results,
/// thinking) the blocks are rendered so the tool name and summary survive instead of being
/// collapsed to a placeholder; otherwise the plain text is used. The `message_id` is
/// positional + stable so event-log replay is deterministic.
pub fn canonical_messages(history: &[PortableMessage], model: &str) -> Vec<CanonicalMessage> {
    history
        .iter()
        .enumerate()
        .map(|(idx, message)| {
            let content = if message.blocks.is_empty() {
                vec![ContentBlock {
                    kind: Some(BlockKind::Text(message.text.clone())),
                    ..ContentBlock::default()
                }]
            } else {
                message
                    .blocks
                    .iter()
                    .map(|block| ContentBlock {
                        kind: Some(BlockKind::Text(render_block(block))),
                        ..ContentBlock::default()
                    })
                    .collect()
            };
            CanonicalMessage {
                message_id: format!("m{idx}"),
                role: message.role.clone(),
                content,
                model: Some(model.to_string()),
                ..CanonicalMessage::default()
            }
        })
        .collect()
}

fn shadow_actor() -> Actor {
    Actor {
        r#type: EnumValue::Known(ActorType::Runner),
        id: "trogon-codex-runner".to_string(),
        ..Actor::default()
    }
}

fn now_timestamp() -> Timestamp {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    Timestamp {
        seconds: elapsed.as_secs() as i64,
        nanos: elapsed.subsec_nanos() as i32,
        ..Default::default()
    }
}

/// Kernel-backed recorder: mirrors a turn's messages into the canonical event log +
/// snapshot. Generic over the kernel backends; constructed once in [`provision`] and used
/// behind `Arc<dyn ShadowRecorder>` so the concrete types never leak into the agent.
struct KernelShadowRecorder<E, S, L> {
    kernel: SessionKernel<E, S, L>,
}

#[async_trait]
impl<E, S, L> ShadowRecorder for KernelShadowRecorder<E, S, L>
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
{
    async fn record_turn(&self, session_id: &str, history: &[PortableMessage], model: &str) {
        let Ok(sid) = SessionId::new(session_id) else {
            return;
        };
        let messages = canonical_messages(history, model);
        if messages.is_empty() {
            return;
        }
        if let Err(err) = self
            .kernel
            .record_conversation(&sid, &messages, shadow_actor(), now_timestamp())
            .await
        {
            tracing::warn!(session_id, error = %err, "codex kernel shadow: record_conversation failed (non-fatal)");
        }
    }
}

/// Provision the canonical shadow recorder when the kernel is enabled. Returns `None`
/// (legacy path, no canonical recording) by default or if provisioning fails — the runner
/// never blocks on the kernel. Gated by both `lease_enabled()` and `snapshot_enabled()`
/// (each folds in the master `session_kernel_enabled` flag).
pub async fn provision(js: &async_nats::jetstream::Context) -> Option<Arc<dyn ShadowRecorder>> {
    let flags = SessionKernelFeatureFlags::default();
    if !flags.lease_enabled() || !flags.snapshot_enabled() {
        return None;
    }

    let config = SessionKernelConfig::default();
    let snapshot_kv = match provision_snapshot_store(js, &config).await {
        Ok(kv) => kv,
        Err(err) => {
            tracing::warn!(error = %err, "codex kernel shadow: snapshot store provisioning failed; disabled");
            return None;
        }
    };
    let lease_kv = match provision_lease_store(js, &config).await {
        Ok(kv) => kv,
        Err(err) => {
            tracing::warn!(error = %err, "codex kernel shadow: lease store provisioning failed; disabled");
            return None;
        }
    };

    let js_client = NatsJetStreamClient::new(js.clone());
    let event_log = EventLog::new(js_client.clone(), js_client, config.clone());
    let operational_policy = SessionKernelOperationalPolicy::default();
    if let Err(err) = event_log
        .provision_stream(&NatsJetStreamClient::new(js.clone()), &operational_policy.nats)
        .await
    {
        tracing::warn!(error = %err, "codex kernel shadow: event-log stream provisioning failed; disabled");
        return None;
    }
    let snapshots = SnapshotStore::new(snapshot_kv, config.clone());
    let leases = SessionLeaseManager::new(SessionKvLeaseFactory::new(lease_kv, &config), "trogon-codex-runner");
    let kernel = SessionKernel::new(config, event_log, snapshots, leases);

    tracing::info!("codex kernel shadow: canonical conversation shadow recording enabled");
    Some(Arc::new(KernelShadowRecorder { kernel }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_messages_preserve_role_text_and_model() {
        let history = vec![
            PortableMessage::text_only("user", "implement the thing"),
            PortableMessage::text_only("assistant", "done"),
        ];
        let canonical = canonical_messages(&history, "o4-mini");
        assert_eq!(canonical.len(), 2);
        assert_eq!(canonical[0].role, "user");
        assert_eq!(canonical[0].message_id, "m0");
        assert_eq!(canonical[1].model.as_deref(), Some("o4-mini"));
        match canonical[1].content[0].kind.as_ref().unwrap() {
            BlockKind::Text(text) => assert_eq!(text, "done"),
            other => panic!("expected text block, got {other:?}"),
        }
    }

    #[test]
    fn canonical_messages_empty_when_no_history() {
        assert!(canonical_messages(&[], "o4-mini").is_empty());
    }

    #[test]
    fn tool_blocks_preserve_name_and_summary_not_placeholder() {
        // §2192 no-lossy: a tool call must keep its name + available summary as canonical
        // truth, not be collapsed to "[tool call]".
        let history = vec![PortableMessage {
            role: "assistant".to_string(),
            text: "[tool call]".to_string(),
            blocks: vec![
                PortableBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "fs_read".to_string(),
                    input_summary: "path=src/lib.rs".to_string(),
                    input: serde_json::Value::Null,
                    parent_tool_use_id: None,
                },
                PortableBlock::ToolResult {
                    id: "t1".to_string(),
                    output_summary: "42 lines".to_string(),
                    output: Some("42 lines".to_string()),
                },
            ],
        }];
        let canonical = canonical_messages(&history, "o4-mini");
        let texts: Vec<&str> = canonical[0]
            .content
            .iter()
            .filter_map(|b| match b.kind.as_ref() {
                Some(BlockKind::Text(t)) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t.contains("fs_read") && t.contains("src/lib.rs")), "got: {texts:?}");
        assert!(texts.iter().any(|t| t.contains("42 lines")), "got: {texts:?}");
        assert!(!texts.iter().any(|t| *t == "[tool call]"), "placeholder must not be canonical truth");
    }
}
