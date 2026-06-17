use buffa::{EnumValue, MessageField};
use buffa_types::google::protobuf::Timestamp;
use serde::Deserialize;
use trogonai_session_contracts::__buffa::oneof::content_block::Kind as BlockKind;
use trogonai_session_contracts::__buffa::oneof::tool_call_result::Kind as ResultKind;
use trogonai_session_contracts::{
    Actor, ActorType, CanonicalMessage, CanonicalToolCall, ContentBlock, SessionConfig, SessionId, SessionMetadata,
    SessionSnapshot, SessionSnapshotState,
};

use crate::error::SessionKernelError;
use crate::event_log::EventLogBackend;
use crate::features::{EventLogPrimaryMode, SessionKernelFeatureFlags};
use crate::kernel::SessionKernel;
use crate::lease::SessionLeaseFactory;
use crate::telemetry;

/// Per-session migration metadata stored alongside canonical snapshots.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionMigrationRecord {
    pub event_log_primary: bool,
    pub migrated_from_legacy: bool,
    pub legacy_message_count: usize,
}

/// Result of a non-blocking shadow sync from legacy export JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowSyncReport {
    pub message_count: usize,
    pub divergence_message_count: usize,
    pub snapshot_saved: bool,
}

/// Divergence between materialized canonical state and the authoritative
/// `state.messages` (§ Migration "comparar snapshot materializado contra estado
/// actual"; § No-Lossy Contract). Conversation roles/counts come from the legacy
/// export; tool I/O fidelity is compared against the full baseline tool calls so
/// summarized/truncated tool input or output is surfaced rather than masked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowDivergenceReport {
    pub materialized_messages: usize,
    pub legacy_messages: usize,
    pub mismatched_roles: usize,
    pub materialized_tool_calls: usize,
    pub baseline_tool_calls: usize,
    pub mismatched_tool_io: usize,
}

#[derive(Debug, Deserialize)]
struct LegacyPortableMessage {
    role: String,
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize)]
struct LegacyPortableExportV2 {
    #[serde(rename = "version")]
    _version: u32,
    messages: Vec<LegacyPortableMessageV2>,
}

#[derive(Debug, Deserialize)]
struct LegacyPortableMessageV2 {
    role: String,
    #[serde(default)]
    blocks: Vec<LegacyPortableBlock>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum LegacyPortableBlock {
    Text { text: String },
    ToolUse { name: String, input_summary: String },
    ToolResult { output_summary: String },
    Thinking { text: String },
}

/// Resolve whether reads should prefer the event log for a session.
pub fn resolve_event_log_primary(flags: &SessionKernelFeatureFlags, record: &SessionMigrationRecord) -> bool {
    match flags.event_log_primary_mode() {
        EventLogPrimaryMode::LegacyMessages => false,
        EventLogPrimaryMode::NewSessionsOnly => record.event_log_primary,
        EventLogPrimaryMode::AllMigrated => record.migrated_from_legacy || record.event_log_primary,
    }
}

/// Parse legacy `session/export` JSON into canonical conversation messages.
pub fn conversation_from_legacy_export(export_json: &str) -> Result<Vec<CanonicalMessage>, SessionKernelError> {
    let trimmed = export_json.trim();
    if trimmed == "null" || trimmed.is_empty() {
        return Ok(Vec::new());
    }

    if trimmed.starts_with('[') {
        let legacy: Vec<LegacyPortableMessage> =
            serde_json::from_str(trimmed).map_err(|err| SessionKernelError::Decode(err.to_string()))?;
        return Ok(legacy
            .into_iter()
            .enumerate()
            .map(|(idx, msg)| legacy_message(&msg.role, &msg.text, idx))
            .collect());
    }

    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|err| SessionKernelError::Decode(err.to_string()))?;
    if value.get("version").and_then(|v| v.as_u64()) == Some(2) {
        let export: LegacyPortableExportV2 =
            serde_json::from_value(value).map_err(|err| SessionKernelError::Decode(err.to_string()))?;
        return Ok(export
            .messages
            .into_iter()
            .enumerate()
            .map(|(idx, msg)| legacy_v2_message(&msg.role, &msg.blocks, idx))
            .collect());
    }

    Err(SessionKernelError::Decode(
        "unsupported legacy export shape".to_string(),
    ))
}

/// Build a materialized snapshot from legacy export without mutating runner state.
pub fn snapshot_from_legacy_export(
    session_id: &SessionId,
    export_json: &str,
    config: Option<SessionConfig>,
    cwd: &str,
) -> Result<SessionSnapshot, SessionKernelError> {
    let conversation = conversation_from_legacy_export(export_json)?;
    let state = SessionSnapshotState {
        session: MessageField::some(SessionMetadata {
            id: session_id.as_str().to_string(),
            cwd: cwd.to_string(),
            ..SessionMetadata::default()
        }),
        config: config.map(MessageField::some).unwrap_or_default(),
        conversation,
        ..SessionSnapshotState::default()
    };

    Ok(SessionSnapshot {
        session_id: session_id.as_str().to_string(),
        last_applied_seq: 0,
        state: MessageField::some(state),
        ..SessionSnapshot::default()
    })
}

/// Extract the textual result content of a canonical tool call, if any. Used to
/// compare tool-output fidelity (a truncated output diverges from the full one).
fn tool_result_text(call: &CanonicalToolCall) -> Option<String> {
    call.result
        .as_option()
        .and_then(|result| result.kind.as_ref())
        .and_then(|kind| match kind {
            ResultKind::Text(text) => Some(text.content.clone()),
            ResultKind::ArtifactRef(_) => None,
        })
}

/// Whether the materialized tool result diverges from the full baseline result.
///
/// A claim-checked `ArtifactRef` stores the FULL output out-of-line (§ No-Lossy
/// Contract: large outputs go to artifact refs without truncating canonical truth), so a
/// materialized ref is NOT a divergence against the legacy inline text — it is the same
/// content stored canonically. Only an inline text result that differs from the full
/// baseline (a summarized/truncated output) counts as divergence.
fn tool_result_diverges(materialized: &CanonicalToolCall, base: &CanonicalToolCall) -> bool {
    let materialized_is_artifact_ref = materialized
        .result
        .as_option()
        .and_then(|result| result.kind.as_ref())
        .is_some_and(|kind| matches!(kind, ResultKind::ArtifactRef(_)));
    if materialized_is_artifact_ref {
        return false;
    }
    tool_result_text(materialized) != tool_result_text(base)
}

/// Compare the materialized canonical state against the authoritative
/// `state.messages` for shadow metrics (§ Migration; § No-Lossy Contract).
///
/// Conversation message count and roles are compared against the legacy export.
/// `baseline_tool_calls` is the FULL tool-call view derived from `state.messages`
/// (full input JSON and output, not the summarized portable export); each
/// materialized tool call is compared against it so a summarized input or a
/// truncated output is reported as a divergence instead of silently coinciding.
pub fn compare_shadow_divergence(
    materialized: &SessionSnapshotState,
    export_json: &str,
    baseline_tool_calls: &[CanonicalToolCall],
) -> Result<ShadowDivergenceReport, SessionKernelError> {
    let legacy = conversation_from_legacy_export(export_json)?;
    let materialized_messages = materialized.conversation.len();
    let legacy_messages = legacy.len();
    let mismatched_roles = materialized
        .conversation
        .iter()
        .zip(legacy.iter())
        .filter(|(left, right)| left.role != right.role)
        .count();

    let materialized_tool_calls = materialized.tool_calls.len();
    let baseline_tool_calls_count = baseline_tool_calls.len();
    let mut mismatched_tool_io = 0usize;
    for base in baseline_tool_calls {
        match materialized.tool_calls.iter().find(|call| call.id == base.id) {
            Some(materialized_call) => {
                if materialized_call.input_json != base.input_json
                    || tool_result_diverges(materialized_call, base)
                {
                    mismatched_tool_io += 1;
                }
            }
            None => mismatched_tool_io += 1,
        }
    }

    if mismatched_roles > 0 || materialized_messages != legacy_messages || mismatched_tool_io > 0 {
        telemetry::metrics::record_shadow_divergence(
            materialized
                .session
                .as_option()
                .map(|meta| meta.id.as_str())
                .unwrap_or("unknown"),
            mismatched_roles + mismatched_tool_io,
        );
    }

    Ok(ShadowDivergenceReport {
        materialized_messages,
        legacy_messages,
        mismatched_roles,
        materialized_tool_calls,
        baseline_tool_calls: baseline_tool_calls_count,
        mismatched_tool_io,
    })
}

/// Shadow-mode sync: materialize legacy export into KV without affecting the
/// operational path. Compares only the conversation (count/roles) here; tool I/O
/// fidelity is compared by the caller AFTER the tool-call events are materialized,
/// so the comparison runs against the COMPLETE snapshot (§ Migration: materialize
/// snapshot → compare). See [`compare_shadow_divergence`].
pub async fn shadow_sync_from_export<E, S, L>(
    kernel: &SessionKernel<E, S, L>,
    session_id: &SessionId,
    export_json: &str,
    cwd: &str,
    portable_config: Option<SessionConfig>,
) -> Result<ShadowSyncReport, SessionKernelError>
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
    let conversation = conversation_from_legacy_export(export_json)?;
    let message_count = conversation.len();

    // Reconstruct the transcript into the event log so the event log — not a
    // directly-built snapshot — is the source of truth (§3). The session metadata
    // (cwd) and portable config aren't carried by transcript events, so patch them
    // onto the materialized snapshot. Empty conversations fall back to the
    // direct snapshot (a zero-length event log has no valid seq to materialize).
    let snapshot = if conversation.is_empty() {
        let snapshot = snapshot_from_legacy_export(session_id, export_json, portable_config, cwd)?;
        kernel.snapshots().save_snapshot(&snapshot).await?;
        snapshot
    } else {
        let mut snapshot = kernel
            .record_conversation(session_id, &conversation, shadow_actor(), Timestamp::default())
            .await?;
        if let Some(state) = snapshot.state.as_option_mut() {
            let session = state.session.get_or_insert_default();
            session.id = session_id.as_str().to_string();
            session.cwd = cwd.to_string();
            if let Some(config) = portable_config {
                state.config = MessageField::some(config);
            }
        }
        kernel.snapshots().save_snapshot(&snapshot).await?;
        snapshot
    };

    // Conversation-only comparison here (the snapshot's tool calls for THIS turn are
    // not materialized yet — the caller compares tool I/O against the complete
    // snapshot afterwards). An empty tool baseline means no tool-I/O divergence.
    let divergence = compare_shadow_divergence(snapshot.state.as_option().expect("snapshot state"), export_json, &[])?;
    telemetry::metrics::record_shadow_sync(session_id.as_str(), message_count);

    Ok(ShadowSyncReport {
        message_count,
        divergence_message_count: divergence.mismatched_roles,
        snapshot_saved: true,
    })
}

/// Shadow-mode sync from a FULL canonical conversation (structured content blocks
/// with complete tool I/O), rather than the summarized portable export. This keeps
/// the canonical transcript no-lossy (§11; § No-Lossy Contract). The conversation is
/// the authoritative `state.messages`, so no divergence comparison is needed.
pub async fn shadow_sync_from_conversation<E, S, L>(
    kernel: &SessionKernel<E, S, L>,
    session_id: &SessionId,
    conversation: &[CanonicalMessage],
    cwd: &str,
    portable_config: Option<SessionConfig>,
) -> Result<ShadowSyncReport, SessionKernelError>
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
    let message_count = conversation.len();

    let snapshot = if conversation.is_empty() {
        let state = SessionSnapshotState {
            session: MessageField::some(SessionMetadata {
                id: session_id.as_str().to_string(),
                cwd: cwd.to_string(),
                ..SessionMetadata::default()
            }),
            config: portable_config.map(MessageField::some).unwrap_or_default(),
            ..SessionSnapshotState::default()
        };
        let snapshot = SessionSnapshot {
            session_id: session_id.as_str().to_string(),
            last_applied_seq: 0,
            state: MessageField::some(state),
            ..SessionSnapshot::default()
        };
        kernel.snapshots().save_snapshot(&snapshot).await?;
        snapshot
    } else {
        let mut snapshot = kernel
            .record_conversation(session_id, conversation, shadow_actor(), Timestamp::default())
            .await?;
        if let Some(state) = snapshot.state.as_option_mut() {
            let session = state.session.get_or_insert_default();
            session.id = session_id.as_str().to_string();
            session.cwd = cwd.to_string();
            if let Some(config) = portable_config {
                state.config = MessageField::some(config);
            }
        }
        kernel.snapshots().save_snapshot(&snapshot).await?;
        snapshot
    };
    let _ = &snapshot;
    telemetry::metrics::record_shadow_sync(session_id.as_str(), message_count);

    Ok(ShadowSyncReport {
        message_count,
        divergence_message_count: 0,
        snapshot_saved: true,
    })
}

fn shadow_actor() -> Actor {
    Actor {
        r#type: EnumValue::Known(ActorType::Kernel),
        id: "shadow-sync".to_string(),
        ..Actor::default()
    }
}

fn legacy_message(role: &str, text: &str, idx: usize) -> CanonicalMessage {
    CanonicalMessage {
        message_id: format!("legacy_msg_{idx}"),
        role: role.to_string(),
        content: vec![ContentBlock {
            kind: Some(BlockKind::Text(text.to_string())),
            ..ContentBlock::default()
        }],
        ..CanonicalMessage::default()
    }
}

fn legacy_v2_message(role: &str, blocks: &[LegacyPortableBlock], idx: usize) -> CanonicalMessage {
    let mut content = Vec::new();
    for block in blocks {
        match block {
            LegacyPortableBlock::Text { text } => {
                content.push(ContentBlock {
                    kind: Some(BlockKind::Text(text.clone())),
                    ..ContentBlock::default()
                });
            }
            LegacyPortableBlock::ToolUse { name, input_summary } => {
                content.push(ContentBlock {
                    kind: Some(BlockKind::Text(format!("[tool:{name}] {input_summary}"))),
                    ..ContentBlock::default()
                });
            }
            LegacyPortableBlock::ToolResult { output_summary } => {
                content.push(ContentBlock {
                    kind: Some(BlockKind::Text(output_summary.clone())),
                    ..ContentBlock::default()
                });
            }
            LegacyPortableBlock::Thinking { text } => {
                content.push(ContentBlock {
                    kind: Some(BlockKind::Text(text.clone())),
                    ..ContentBlock::default()
                });
            }
        }
    }
    if content.is_empty() {
        content.push(ContentBlock {
            kind: Some(BlockKind::Text(String::new())),
            ..ContentBlock::default()
        });
    }
    CanonicalMessage {
        message_id: format!("legacy_msg_{idx}"),
        role: role.to_string(),
        content,
        ..CanonicalMessage::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v1_export_array() {
        let msgs = conversation_from_legacy_export(r#"[{"role":"user","text":"hello"}]"#).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");
    }

    #[test]
    fn event_log_primary_legacy_by_default() {
        let flags = SessionKernelFeatureFlags::default();
        let record = SessionMigrationRecord::default();
        assert!(!resolve_event_log_primary(&flags, &record));
    }

    fn tool_call(id: &str, input_json: &str) -> CanonicalToolCall {
        CanonicalToolCall {
            id: id.to_string(),
            tool_execution_id: id.to_string(),
            name: "bash".to_string(),
            input_json: input_json.to_string(),
            ..CanonicalToolCall::default()
        }
    }

    #[test]
    fn shadow_divergence_detects_summarized_tool_input() {
        // Materialized snapshot holds the lossy (summarized) input; the baseline is
        // the full input from state.messages. The comparison must surface the loss.
        let materialized = SessionSnapshotState {
            tool_calls: vec![tool_call("t1", "{\"cmd\":\"cargo te…")],
            ..SessionSnapshotState::default()
        };
        let baseline = vec![tool_call("t1", "{\"cmd\":\"cargo test --workspace --all-features\"}")];

        let report = compare_shadow_divergence(&materialized, "[]", &baseline).unwrap();
        assert_eq!(report.baseline_tool_calls, 1);
        assert_eq!(
            report.mismatched_tool_io, 1,
            "a summarized input must diverge from the full state.messages baseline"
        );
    }

    #[test]
    fn shadow_divergence_clean_when_tool_io_matches() {
        let call = tool_call("t1", "{\"cmd\":\"ls\"}");
        let materialized = SessionSnapshotState {
            tool_calls: vec![call.clone()],
            ..SessionSnapshotState::default()
        };
        let report = compare_shadow_divergence(&materialized, "[]", &[call]).unwrap();
        assert_eq!(report.mismatched_tool_io, 0);
    }

    #[test]
    fn shadow_divergence_treats_claim_checked_artifact_ref_as_preserved() {
        use buffa::MessageField;
        use trogonai_session_contracts::{ArtifactRef, TextToolResult, ToolCallResult};

        // Baseline (legacy state.messages) holds the full inline text output.
        let mut base = tool_call("t1", "{\"cmd\":\"ls\"}");
        base.result = MessageField::some(ToolCallResult {
            kind: Some(
                TextToolResult {
                    content: "a very large tool output...".to_string(),
                    ..TextToolResult::default()
                }
                .into(),
            ),
            ..ToolCallResult::default()
        });

        // Materialized stored the same output out-of-line as a claim-checked artifact
        // ref (§ No-Lossy: large outputs become artifact refs without truncating truth).
        let mut materialized_call = tool_call("t1", "{\"cmd\":\"ls\"}");
        materialized_call.result = MessageField::some(ToolCallResult {
            kind: Some(
                ArtifactRef {
                    artifact_id: "art_1".to_string(),
                    ..ArtifactRef::default()
                }
                .into(),
            ),
            ..ToolCallResult::default()
        });
        let materialized = SessionSnapshotState {
            tool_calls: vec![materialized_call],
            ..SessionSnapshotState::default()
        };

        let report = compare_shadow_divergence(&materialized, "[]", &[base]).unwrap();
        assert_eq!(
            report.mismatched_tool_io, 0,
            "a claim-checked artifact ref preserves the full output and must not diverge"
        );
    }
}
