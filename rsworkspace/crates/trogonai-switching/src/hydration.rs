use serde_json::json;
use trogonai_session_contracts::{
    CanonicalMessage, PromptProjection, SessionConfig, SessionSnapshotState,
    __buffa::oneof::content_block::Kind as BlockKind,
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
        mcp_servers_json: config
            .map(|cfg| cfg.mcp_servers_json.clone())
            .unwrap_or_default(),
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
    let blocks: Vec<serde_json::Value> = message
        .content
        .iter()
        .filter_map(|block| match block.kind.as_ref()? {
            BlockKind::Text(text) => Some(json!({"type":"text","text": text})),
            _ => None,
        })
        .collect();
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

/// Merge portable config from runner legacy state into canonical `SessionConfig`.
pub fn merge_portable_config(
    base: &mut SessionConfig,
    portable: &PortableRunnerConfig,
) {
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
        assert_eq!(
            portable.compactor_model.as_deref(),
            Some("xai/grok-code-fast")
        );
        assert_eq!(portable.system_prompt.as_deref(), Some("stay helpful"));
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
