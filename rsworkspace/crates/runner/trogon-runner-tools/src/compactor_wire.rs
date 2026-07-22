//! Protobuf wire encoding for compactor request/response (M2/M3).

use buffa::Message as _;
use trogon_tools::{ContentBlock, Message};
use trogonai_compactor_proto::{
    __buffa::oneof::content_block::Kind as BlockKind, CompactError as ProtoCompactError,
    CompactRequest as ProtoRequest, CompactResponse as ProtoResponse, ContentBlock as ProtoBlock,
    Message as ProtoMessage,
};

use crate::compaction::CompactError;

#[derive(Debug, Clone)]
pub struct CompactWireResponse {
    pub messages: Vec<Message>,
    pub compacted: bool,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub kept_count: usize,
    pub fallback_model: Option<String>,
}

pub fn encode_compact_request(
    messages: &[Message],
    provider: &str,
    model: &str,
    context_window: Option<u64>,
    compactor_provider: Option<&str>,
    compactor_model: Option<&str>,
) -> Vec<u8> {
    let proto = ProtoRequest {
        messages: messages.iter().map(message_to_proto).collect(),
        provider: provider.to_string(),
        model: model.to_string(),
        context_window: context_window.unwrap_or(0),
        compactor_provider: compactor_provider.map(str::to_string),
        compactor_model: compactor_model.map(str::to_string),
        __buffa_unknown_fields: Default::default(),
    };
    proto.encode_to_vec()
}

pub fn decode_compact_response(payload: &[u8]) -> Result<CompactWireResponse, CompactError> {
    if let Ok(err) = ProtoCompactError::decode_from_slice(payload)
        && !err.error.is_empty()
    {
        return Err(CompactError::InvalidResponse(err.error));
    }

    let proto = ProtoResponse::decode_from_slice(payload).map_err(|e| CompactError::InvalidResponse(e.to_string()))?;

    Ok(CompactWireResponse {
        messages: proto.messages.iter().filter_map(proto_to_message).collect(),
        compacted: proto.compacted,
        tokens_before: proto.tokens_before as usize,
        tokens_after: proto.tokens_after as usize,
        kept_count: proto.kept_count as usize,
        fallback_model: proto.fallback_model.filter(|s| !s.is_empty()),
    })
}

fn proto_to_message(m: &ProtoMessage) -> Option<Message> {
    Some(Message {
        role: m.role.clone(),
        content: m.content.iter().filter_map(proto_to_block).collect(),
    })
}

fn proto_to_block(b: &ProtoBlock) -> Option<ContentBlock> {
    match b.kind.as_ref()? {
        BlockKind::Text(t) => Some(ContentBlock::Text { text: t.text.clone() }),
        BlockKind::Image(i) => {
            let source: trogon_tools::ImageSource =
                serde_json::from_str(&i.source_json).unwrap_or(trogon_tools::ImageSource::Url { url: String::new() });
            Some(ContentBlock::Image { source })
        }
        BlockKind::Thinking(t) => Some(ContentBlock::Thinking {
            thinking: t.thinking.clone(),
            signature: None,
        }),
        BlockKind::ToolUse(t) => {
            let input = serde_json::from_str(&t.input_json).unwrap_or(serde_json::json!({}));
            Some(ContentBlock::ToolUse {
                id: t.id.clone(),
                name: t.name.clone(),
                input,
                parent_tool_use_id: t.parent_tool_use_id.clone(),
            })
        }
        BlockKind::ToolResult(t) => Some(ContentBlock::ToolResult {
            tool_use_id: t.tool_use_id.clone(),
            content: t.content.clone(),
            blocks: vec![],
        }),
    }
}

fn message_to_proto(m: &Message) -> ProtoMessage {
    ProtoMessage {
        role: m.role.clone(),
        content: m.content.iter().map(block_to_proto).collect(),
        __buffa_unknown_fields: Default::default(),
    }
}

fn block_to_proto(b: &ContentBlock) -> ProtoBlock {
    let kind = match b {
        ContentBlock::Text { text } => BlockKind::Text(Box::new(trogonai_compactor_proto::TextBlock {
            text: text.clone(),
            __buffa_unknown_fields: Default::default(),
        })),
        ContentBlock::Image { source } => BlockKind::Image(Box::new(trogonai_compactor_proto::ImageBlock {
            source_json: serde_json::to_string(source).unwrap_or_else(|_| "{}".into()),
            __buffa_unknown_fields: Default::default(),
        })),
        ContentBlock::Thinking { thinking, .. } => BlockKind::Thinking(Box::new(trogonai_compactor_proto::ThinkingBlock {
            thinking: thinking.clone(),
            __buffa_unknown_fields: Default::default(),
        })),
        ContentBlock::ToolUse {
            id,
            name,
            input,
            parent_tool_use_id,
        } => BlockKind::ToolUse(Box::new(trogonai_compactor_proto::ToolUseBlock {
            id: id.clone(),
            name: name.clone(),
            input_json: input.to_string(),
            parent_tool_use_id: parent_tool_use_id.clone(),
            __buffa_unknown_fields: Default::default(),
        })),
        ContentBlock::ToolResult { tool_use_id, content, .. } => {
            BlockKind::ToolResult(Box::new(trogonai_compactor_proto::ToolResultBlock {
                tool_use_id: tool_use_id.clone(),
                content: content.clone(),
                __buffa_unknown_fields: Default::default(),
            }))
        }
    };
    ProtoBlock {
        kind: Some(kind),
        __buffa_unknown_fields: Default::default(),
    }
}
