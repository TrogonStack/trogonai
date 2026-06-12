use buffa::{EnumValue, MessageField};
use buffa_types::google::protobuf::Timestamp;
use trogonai_session_contracts::__buffa::oneof::content_block::Kind as BlockKind;
use serde::Deserialize;
use trogonai_session_contracts::{
    Actor, ActorType, CanonicalMessage, ContentBlock, SessionConfig, SessionId, SessionMetadata,
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

/// Divergence between materialized canonical state and legacy export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowDivergenceReport {
    pub materialized_messages: usize,
    pub legacy_messages: usize,
    pub mismatched_roles: usize,
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
pub fn resolve_event_log_primary(
    flags: &SessionKernelFeatureFlags,
    record: &SessionMigrationRecord,
) -> bool {
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

/// Compare materialized canonical conversation against legacy export for shadow metrics.
pub fn compare_shadow_divergence(
    materialized: &SessionSnapshotState,
    export_json: &str,
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

    if mismatched_roles > 0 || materialized_messages != legacy_messages {
        telemetry::metrics::record_shadow_divergence(
            materialized
                .session
                .as_option()
                .map(|meta| meta.id.as_str())
                .unwrap_or("unknown"),
            mismatched_roles,
        );
    }

    Ok(ShadowDivergenceReport {
        materialized_messages,
        legacy_messages,
        mismatched_roles,
    })
}

/// Shadow-mode sync: materialize legacy export into KV without affecting the operational path.
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

    let divergence = compare_shadow_divergence(
        snapshot.state.as_option().expect("snapshot state"),
        export_json,
    )?;
    telemetry::metrics::record_shadow_sync(session_id.as_str(), message_count);

    Ok(ShadowSyncReport {
        message_count,
        divergence_message_count: divergence.mismatched_roles,
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
}
