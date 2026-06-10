//! Protobuf wire encoding for compactor request/response (M1).

use buffa::Message as _;
use trogonai_compactor_proto::{
    __buffa::oneof::content_block::Kind as BlockKind, CompactError as ProtoCompactError,
    CompactRequest as ProtoRequest, CompactResponse as ProtoResponse, ContentBlock as ProtoBlock,
    Message as ProtoMessage,
};

use crate::error::CompactorError;
use crate::types::{ContentBlock, Message};

#[derive(Debug, Clone)]
pub struct CompactRequest {
    pub messages: Vec<Message>,
    pub provider: String,
    pub model: Option<String>,
    pub context_window: Option<u64>,
    pub compactor_provider: Option<String>,
    pub compactor_model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CompactResponse {
    pub messages: Vec<Message>,
    pub compacted: bool,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub kept_count: usize,
    pub fallback_model: Option<String>,
}

pub fn decode_request(payload: &[u8]) -> Result<CompactRequest, CompactorError> {
    let proto = ProtoRequest::decode_from_slice(payload).map_err(|e| CompactorError::InvalidRequest(e.to_string()))?;
    Ok(CompactRequest {
        messages: proto.messages.iter().map(proto_to_message).collect(),
        provider: proto.provider,
        model: optional_string(proto.model),
        context_window: optional_u64(proto.context_window),
        compactor_provider: proto.compactor_provider.filter(|s| !s.is_empty()),
        compactor_model: proto.compactor_model.filter(|s| !s.is_empty()),
    })
}

pub fn encode_response(resp: &CompactResponse) -> Vec<u8> {
    let proto = ProtoResponse {
        messages: resp.messages.iter().map(message_to_proto).collect(),
        compacted: resp.compacted,
        tokens_before: resp.tokens_before as u64,
        tokens_after: resp.tokens_after as u64,
        kept_count: resp.kept_count as u32,
        fallback_model: resp.fallback_model.clone(),
        __buffa_unknown_fields: Default::default(),
    };
    proto.encode_to_vec()
}

pub fn encode_error(err: &CompactorError) -> Vec<u8> {
    ProtoCompactError {
        error: err.to_string(),
        __buffa_unknown_fields: Default::default(),
    }
    .encode_to_vec()
}

fn optional_string(s: String) -> Option<String> {
    if s.is_empty() { None } else { Some(s) }
}

fn optional_u64(v: u64) -> Option<u64> {
    if v == 0 { None } else { Some(v) }
}

fn proto_to_message(m: &ProtoMessage) -> Message {
    Message {
        role: m.role.clone(),
        content: m.content.iter().filter_map(proto_to_block).collect(),
    }
}

fn proto_to_block(b: &ProtoBlock) -> Option<ContentBlock> {
    match b.kind.as_ref()? {
        BlockKind::Text(t) => Some(ContentBlock::Text { text: t.text.clone() }),
        BlockKind::Image(i) => {
            let source = serde_json::from_str(&i.source_json).unwrap_or(serde_json::json!({}));
            Some(ContentBlock::Image { source })
        }
        BlockKind::Thinking(t) => Some(ContentBlock::Thinking {
            thinking: t.thinking.clone(),
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
        }),
    }
}

pub(crate) fn message_to_proto(m: &Message) -> ProtoMessage {
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
            source_json: source.to_string(),
            __buffa_unknown_fields: Default::default(),
        })),
        ContentBlock::Thinking { thinking } => BlockKind::Thinking(Box::new(trogonai_compactor_proto::ThinkingBlock {
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
        ContentBlock::ToolResult { tool_use_id, content } => {
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
