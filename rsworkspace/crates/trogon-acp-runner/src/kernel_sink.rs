//! Optional Session Kernel conversation sink.
//!
//! When the Session Kernel is enabled, the runner mirrors each turn's conversation
//! into the canonical event log in shadow mode (§3 event log, § Migration). This is
//! best-effort and strictly additive: the legacy `NatsSessionStore` remains the
//! authoritative store, and the default runner path (kernel disabled) is unchanged.

use async_trait::async_trait;
use buffa::{EnumValue, MessageField};
use trogonai_session_contracts::{
    Actor, ActorType, CanonicalToolCall, SessionConfig, SessionId, TextToolResult, ToolCallResult,
    ToolCallStatus,
};
use trogonai_session_kernel::{
    EventLogBackend, SessionKernel, SessionLeaseFactory, shadow_sync_from_export,
};

/// Parse a V2 conversation export into canonical tool calls (best-effort; tool
/// input/output are the export's summaries, so this is lossy by design — fidelity
/// suited to the shadow path).
fn tool_calls_from_v2_export(export_json: &str) -> Vec<CanonicalToolCall> {
    use std::collections::HashMap;
    use trogon_runner_tools::portable_session::{PortableBlock, PortableExportV2};

    let Ok(export) = serde_json::from_str::<PortableExportV2>(export_json) else {
        return Vec::new();
    };
    let mut calls: Vec<CanonicalToolCall> = Vec::new();
    let mut results: HashMap<String, String> = HashMap::new();
    for message in &export.messages {
        for block in &message.blocks {
            match block {
                PortableBlock::ToolUse { id, name, input_summary } => {
                    calls.push(CanonicalToolCall {
                        id: id.clone(),
                        tool_execution_id: id.clone(),
                        name: name.clone(),
                        input_json: input_summary.clone(),
                        ..CanonicalToolCall::default()
                    });
                }
                PortableBlock::ToolResult { id, output_summary } => {
                    results.insert(id.clone(), output_summary.clone());
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

fn shadow_actor() -> Actor {
    Actor {
        r#type: EnumValue::Known(ActorType::Kernel),
        id: "acp-runner-shadow".to_string(),
        ..Actor::default()
    }
}

/// Best-effort mirror of the session conversation into the canonical event log.
///
/// Implementations MUST be non-fatal: they log and swallow errors so the live
/// prompt path is never affected by kernel issues.
#[async_trait]
pub trait ConversationSink: Send + Sync {
    async fn sync(&self, session_id: &str, export_json: &str, cwd: &str);
}

/// Kernel-backed sink: reconstructs the event log from the legacy conversation
/// export via the shadow-sync path (idempotent — only newly-grown turns append).
pub struct KernelConversationSink<E, S, L> {
    kernel: SessionKernel<E, S, L>,
}

impl<E, S, L> KernelConversationSink<E, S, L> {
    pub fn new(kernel: SessionKernel<E, S, L>) -> Self {
        Self { kernel }
    }
}

#[async_trait]
impl<E, S, L> ConversationSink for KernelConversationSink<E, S, L>
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
    L: SessionLeaseFactory + Clone + 'static,
{
    async fn sync(&self, session_id: &str, export_json: &str, cwd: &str) {
        let Ok(sid) = SessionId::new(session_id) else {
            return;
        };
        // Mirror the conversation (messages) into the event log.
        if let Err(err) =
            shadow_sync_from_export(&self.kernel, &sid, export_json, cwd, None::<SessionConfig>)
                .await
        {
            tracing::warn!(session_id, error = %err, "kernel conversation sink: shadow sync failed");
            return;
        }
        // Reconstruct structured tool-call events from the V2 export.
        let tool_calls = tool_calls_from_v2_export(export_json);
        if !tool_calls.is_empty()
            && let Err(err) = self
                .kernel
                .record_tool_calls(&sid, &tool_calls, shadow_actor(), Default::default())
                .await
        {
            tracing::warn!(session_id, error = %err, "kernel conversation sink: tool-call sync failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default, Clone)]
    struct RecordingSink {
        calls: Arc<Mutex<Vec<(String, String)>>>,
    }

    #[async_trait]
    impl ConversationSink for RecordingSink {
        async fn sync(&self, session_id: &str, export_json: &str, _cwd: &str) {
            self.calls
                .lock()
                .unwrap()
                .push((session_id.to_string(), export_json.to_string()));
        }
    }

    #[tokio::test]
    async fn conversation_sink_is_invoked_with_export() {
        let sink = RecordingSink::default();
        sink.sync("sess_1", r#"[{"role":"user","text":"hi"}]"#, "/repo").await;
        let calls = sink.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "sess_1");
        assert!(calls[0].1.contains("user"));
    }

    #[test]
    fn parses_tool_calls_from_v2_export() {
        let export = r#"{
            "version": 2,
            "messages": [
                {"version":2,"role":"assistant","blocks":[
                    {"type":"tool_use","id":"t1","name":"bash","input_summary":"{\"cmd\":\"ls\"}"}
                ]},
                {"version":2,"role":"user","blocks":[
                    {"type":"tool_result","id":"t1","output_summary":"file.txt"}
                ]}
            ]
        }"#;
        let calls = tool_calls_from_v2_export(export);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "t1");
        assert_eq!(calls[0].name, "bash");
        assert!(calls[0].result.as_option().is_some());
    }

    #[test]
    fn tool_calls_from_v2_export_handles_non_v2() {
        assert!(tool_calls_from_v2_export(r#"[{"role":"user","text":"hi"}]"#).is_empty());
        assert!(tool_calls_from_v2_export("not json").is_empty());
    }
}
