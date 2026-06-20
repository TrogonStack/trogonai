use serde_json::json;
use trogonai_session_contracts::{
    __buffa::oneof::content_block::Kind as BlockKind, CanonicalMessage, ContentBlock, PromptProjection, SessionConfig,
    SessionSnapshotState,
};

use crate::error::SwitchingError;

/// Portable runner configuration preserved across model switches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PortableRunnerConfig {
    pub compactor_model: Option<String>,
    pub mode: Option<String>,
    pub system_prompt: Option<String>,
    pub mcp_servers_json: Vec<String>,
}

/// Extract portable runner configuration from canonical snapshot state.
pub fn portable_config_from_snapshot(state: &SessionSnapshotState) -> PortableRunnerConfig {
    let config = state.config.as_option();
    PortableRunnerConfig {
        compactor_model: config.and_then(|cfg| cfg.compactor_model.clone()),
        mode: None,
        system_prompt: config.and_then(|cfg| cfg.system_prompt.clone()),
        mcp_servers_json: config.map(|cfg| cfg.mcp_servers_json.clone()).unwrap_or_default(),
    }
}

/// Build `session/import` messages JSON from projection and canonical conversation.
pub fn messages_json_for_runner_hydration(
    projection: &PromptProjection,
    fallback_conversation: &[CanonicalMessage],
) -> Result<String, SwitchingError> {
    if !projection.included_blocks.is_empty() {
        let messages = projection_blocks_to_legacy_messages(projection);
        return serde_json::to_string(&json!({
            "version": 2,
            "messages": messages,
        }))
        .map_err(|err| SwitchingError::Decode(err.to_string()));
    }

    let messages: Vec<serde_json::Value> = fallback_conversation
        .iter()
        .map(canonical_message_to_legacy_json)
        .collect();
    serde_json::to_string(&messages).map_err(|err| SwitchingError::Decode(err.to_string()))
}

fn projection_blocks_to_legacy_messages(projection: &PromptProjection) -> Vec<serde_json::Value> {
    projection
        .included_blocks
        .iter()
        .filter_map(|block| {
            let texts = block
                .content
                .iter()
                .filter_map(|content| match content.kind.as_ref()? {
                    BlockKind::Text(text) => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            if texts.is_empty() {
                return None;
            }
            let role = match block.kind.as_known() {
                Some(trogonai_session_contracts::ProjectionBlockKind::CurrentRequest)
                | Some(trogonai_session_contracts::ProjectionBlockKind::RecentTurn) => "user",
                _ => "assistant",
            };
            Some(json!({
                "version": 2,
                "role": role,
                "blocks": texts.into_iter().map(|text| json!({"type":"text","text": text})).collect::<Vec<_>>(),
            }))
        })
        .collect()
}

fn canonical_message_to_legacy_json(message: &CanonicalMessage) -> serde_json::Value {
    let blocks: Vec<serde_json::Value> = message.content.iter().filter_map(content_block_to_portable_json).collect();
    if blocks.is_empty() {
        json!({"role": message.role, "text": ""})
    } else {
        json!({
            "version": 2,
            "role": message.role,
            "blocks": blocks,
        })
    }
}

/// Serialize a canonical `ContentBlock` into the V2 `session/import` PortableBlock JSON.
/// Tool calls and reasoning must NOT be silently dropped during hydration: a tools-capable
/// destination receives structured `tool_use`/`tool_result` blocks (cambio-modelo.md §637
/// "un modelo con tools recibe tool calls estructurados"; §1818 "tool calls completadas se
/// preservan"); the destination runner textualizes them on import when it is text-only.
fn content_block_to_portable_json(block: &ContentBlock) -> Option<serde_json::Value> {
    match block.kind.as_ref()? {
        BlockKind::Text(text) => Some(json!({"type": "text", "text": text})),
        BlockKind::Thinking(text) => Some(json!({"type": "thinking", "text": text})),
        BlockKind::ToolUse(tool_use) => {
            // Carry the FULL structured input (parsed from canonical `input_json`) plus the
            // parent linkage so a tools-capable destination receives structured tool calls
            // (§637/§11), not a flattened string. `input_summary` is kept for N-1 readers.
            let structured_input =
                serde_json::from_str::<serde_json::Value>(&tool_use.input_json).unwrap_or(serde_json::Value::Null);
            let mut value = json!({
                "type": "tool_use",
                "id": tool_use.id,
                "name": tool_use.name,
                "input_summary": tool_use.input_json,
                "input": structured_input,
            });
            if let Some(parent) = &tool_use.parent_tool_use_id {
                value["parent_tool_use_id"] = json!(parent);
            }
            Some(value)
        }
        BlockKind::ToolResult(tool_result) => Some(json!({
            "type": "tool_result",
            "id": tool_result.tool_use_id,
            "output_summary": render_tool_result_text(&tool_result.result),
        })),
        // The portable format has no native image block; preserve a marker so the turn is
        // not dropped (the canonical artifact ref remains in the kernel snapshot).
        BlockKind::ImageRef(_) => Some(json!({"type": "text", "text": "[image]"})),
    }
}

fn render_tool_result_text(
    result: &buffa::MessageField<trogonai_session_contracts::ToolCallResult>,
) -> String {
    use trogonai_session_contracts::__buffa::oneof::tool_call_result::Kind as ToolResultKind;
    result
        .as_option()
        .and_then(|result| match result.kind.as_ref() {
            Some(ToolResultKind::Text(text)) => Some(text.content.clone()),
            Some(ToolResultKind::ArtifactRef(artifact)) => {
                Some(format!("artifact {} preview {}", artifact.artifact_id, artifact.preview))
            }
            None => None,
        })
        .unwrap_or_default()
}

/// Merge portable config from runner legacy state into canonical `SessionConfig`.
pub fn merge_portable_config(base: &mut SessionConfig, portable: &PortableRunnerConfig) {
    if let Some(compactor_model) = &portable.compactor_model {
        base.compactor_model = Some(compactor_model.clone());
    }
    if let Some(system_prompt) = &portable.system_prompt {
        base.system_prompt = Some(system_prompt.clone());
    }
    if !portable.mcp_servers_json.is_empty() {
        base.mcp_servers_json = portable.mcp_servers_json.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buffa::{EnumValue, MessageField};
    use trogonai_session_contracts::{ContentBlock, ProjectionBlock, ProjectionBlockKind};

    #[test]
    fn portable_config_extracts_compactor_model() {
        let state = SessionSnapshotState {
            config: MessageField::some(SessionConfig {
                compactor_model: Some("xai/grok-code-fast".to_string()),
                system_prompt: Some("stay helpful".to_string()),
                ..SessionConfig::default()
            }),
            ..SessionSnapshotState::default()
        };
        let portable = portable_config_from_snapshot(&state);
        assert_eq!(portable.compactor_model.as_deref(), Some("xai/grok-code-fast"));
        assert_eq!(portable.system_prompt.as_deref(), Some("stay helpful"));
    }

    #[test]
    fn canonical_hydration_preserves_tool_calls_not_just_text() {
        use trogonai_session_contracts::{ToolResultBlock, ToolUseBlock};
        // An assistant turn that is a tool call plus the matching tool result turn.
        // The destination runner must receive the tool call (structured), not have it
        // silently dropped (cambio-modelo.md §637 "un modelo con tools recibe tool calls
        // estructurados"; §1818 "tool calls completadas se preservan").
        let conversation = vec![
            CanonicalMessage {
                message_id: "m_tool".to_string(),
                role: "assistant".to_string(),
                content: vec![ContentBlock {
                    kind: Some(BlockKind::ToolUse(Box::new(ToolUseBlock {
                        id: "toolu_1".to_string(),
                        name: "bash".to_string(),
                        input_json: r#"{"cmd":"cargo test"}"#.to_string(),
                        ..ToolUseBlock::default()
                    }))),
                    ..ContentBlock::default()
                }],
                ..CanonicalMessage::default()
            },
            CanonicalMessage {
                message_id: "m_res".to_string(),
                role: "user".to_string(),
                content: vec![ContentBlock {
                    kind: Some(BlockKind::ToolResult(Box::new(ToolResultBlock {
                        tool_use_id: "toolu_1".to_string(),
                        ..ToolResultBlock::default()
                    }))),
                    ..ContentBlock::default()
                }],
                ..CanonicalMessage::default()
            },
        ];

        let json = messages_json_for_runner_hydration(&PromptProjection::default(), &conversation).unwrap();
        // The tool call must survive hydration in some form (structured tool_use block),
        // and the roles must be preserved from the canonical conversation.
        assert!(
            json.contains("toolu_1") && json.contains("bash"),
            "tool call must reach the destination runner, not be dropped; got: {json}"
        );
        assert!(json.contains("tool_use"), "tool_use block must be serialized; got: {json}");
        assert!(json.contains("tool_result"), "tool_result block must be serialized; got: {json}");
    }

    #[test]
    fn projection_messages_json_is_v2_export() {
        let projection = PromptProjection {
            included_blocks: vec![ProjectionBlock {
                kind: EnumValue::Known(ProjectionBlockKind::CurrentRequest),
                content: vec![ContentBlock {
                    kind: Some(BlockKind::Text("continue".to_string())),
                    ..ContentBlock::default()
                }],
                ..ProjectionBlock::default()
            }],
            ..PromptProjection::default()
        };
        let json = messages_json_for_runner_hydration(&projection, &[]).unwrap();
        assert!(json.contains("\"version\":2"));
        assert!(json.contains("continue"));
    }
}
