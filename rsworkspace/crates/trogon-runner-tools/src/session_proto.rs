//! Protobuf encoding for `SessionState` persistence (M3, ADR 0009).
//!
//! The NATS KV value for an ACP session is encoded from the versioned
//! `trogonai.session.v1.SessionRecord` contract, replacing the previous serde
//! JSON encoding. Migration of pre-protobuf records is C4 (separate).

use buffa::Message as _;
use trogon_tools::{ContentBlock, ImageSource, Message};
use trogonai_session_proto as sp;
// `SessionRecord.messages` uses the compactor `Message` that session-proto
// regenerates from the imported proto — use those types, not compactor-proto's.
use trogonai_session_proto::compactor as cp;
use trogonai_session_proto::compactor::__buffa::oneof::content_block::Kind as BlockKind;

use crate::egress::{EgressAction, EgressPolicy, EgressRule};
use crate::session_store::{
    AuditEntry, AuditOutcome, PolicyAction, SessionState, StoredMcpServer, TodoItem, ToolPolicy,
};

/// Encode session state to protobuf bytes for NATS KV.
pub fn encode(state: &SessionState) -> Vec<u8> {
    to_proto(state).encode_to_vec()
}

/// Decode session state from protobuf bytes.
pub fn decode(bytes: &[u8]) -> anyhow::Result<SessionState> {
    let rec = sp::SessionRecord::decode_from_slice(bytes)
        .map_err(|e| anyhow::anyhow!("session record protobuf decode failed: {e}"))?;
    Ok(from_proto(rec))
}

// ── Standalone compaction override (xai / openrouter) ──────────────────────────
//
// The xai/openrouter `SESSIONS` bucket is a JSON contract shared with
// trogon-console (browser-facing interop boundary, ADR 0009 JSON exception), so
// its session blob stays JSON. The compaction override — the runner-internal data
// this plan adds — is persisted separately as a versioned `CompactionConfig`
// protobuf record under a sibling key (ADR 0009), keeping the console JSON intact.

/// Encode a compaction override `(provider, model)` to protobuf bytes.
pub fn encode_compaction(provider: Option<&str>, model: Option<&str>) -> Vec<u8> {
    sp::CompactionConfig {
        compactor_model: model.map(str::to_string),
        compactor_provider: provider.map(str::to_string),
        __buffa_unknown_fields: Default::default(),
    }
    .encode_to_vec()
}

/// Decode a compaction override record → `(provider, model)`. Malformed/missing → `(None, None)`.
pub fn decode_compaction(bytes: &[u8]) -> (Option<String>, Option<String>) {
    match sp::CompactionConfig::decode_from_slice(bytes) {
        Ok(c) => (c.compactor_provider, c.compactor_model),
        Err(_) => (None, None),
    }
}

// ── SessionState ↔ SessionRecord ───────────────────────────────────────────────

fn to_proto(s: &SessionState) -> sp::SessionRecord {
    sp::SessionRecord {
        messages: s.messages.iter().map(message_to_proto).collect(),
        model: s.model.clone(),
        compaction: buffa::MessageField::some(sp::CompactionConfig {
            compactor_model: s.compactor_model.clone(),
            compactor_provider: s.compactor_provider.clone(),
            __buffa_unknown_fields: Default::default(),
        }),
        mode: s.mode.clone(),
        cwd: s.cwd.clone(),
        created_at: s.created_at.clone(),
        updated_at: s.updated_at.clone(),
        title: s.title.clone(),
        system_prompt: s.system_prompt.clone(),
        disable_builtin_tools: s.disable_builtin_tools,
        parent_session_id: s.parent_session_id.clone(),
        branched_at_index: s.branched_at_index.map(|i| i as u64),
        mcp_servers: s.mcp_servers.iter().map(mcp_to_proto).collect(),
        additional_roots: s.additional_roots.clone(),
        allowed_tools: s.allowed_tools.clone(),
        tool_policies: s.tool_policies.iter().map(tool_policy_to_proto).collect(),
        egress_policy: match &s.egress_policy {
            Some(p) => buffa::MessageField::some(egress_to_proto(p)),
            None => buffa::MessageField::none(),
        },
        audit_log: s.audit_log.iter().map(audit_to_proto).collect(),
        terminal_id: s.terminal_id.clone(),
        terminal_cwd: s.terminal_cwd.clone(),
        todos: s.todos.iter().map(todo_to_proto).collect(),
        permission_rules_text: s.permission_rules_text.clone(),
        spawn_depth: s.spawn_depth,
        token_budget: s.token_budget,
        total_input_tokens: s.total_input_tokens,
        total_output_tokens: s.total_output_tokens,
        total_cache_creation_tokens: s.total_cache_creation_tokens,
        total_cache_read_tokens: s.total_cache_read_tokens,
        __buffa_unknown_fields: Default::default(),
    }
}

fn from_proto(r: sp::SessionRecord) -> SessionState {
    let (compactor_model, compactor_provider) = r
        .compaction
        .as_option()
        .map(|c| (c.compactor_model.clone(), c.compactor_provider.clone()))
        .unwrap_or((None, None));
    SessionState {
        messages: r.messages.iter().filter_map(proto_to_message).collect(),
        model: r.model,
        compactor_provider,
        compactor_model,
        mode: r.mode,
        cwd: r.cwd,
        created_at: r.created_at,
        updated_at: r.updated_at,
        title: r.title,
        mcp_servers: r.mcp_servers.iter().map(mcp_from_proto).collect(),
        system_prompt: r.system_prompt,
        additional_roots: r.additional_roots,
        disable_builtin_tools: r.disable_builtin_tools,
        allowed_tools: r.allowed_tools,
        parent_session_id: r.parent_session_id,
        branched_at_index: r.branched_at_index.map(|i| i as usize),
        tool_policies: r.tool_policies.iter().map(tool_policy_from_proto).collect(),
        egress_policy: r.egress_policy.as_option().map(egress_from_proto),
        audit_log: r.audit_log.iter().map(audit_from_proto).collect(),
        terminal_id: r.terminal_id,
        terminal_cwd: r.terminal_cwd,
        todos: r.todos.iter().map(todo_from_proto).collect(),
        permission_rules_text: r.permission_rules_text,
        spawn_depth: r.spawn_depth,
        token_budget: r.token_budget,
        total_input_tokens: r.total_input_tokens,
        total_output_tokens: r.total_output_tokens,
        total_cache_creation_tokens: r.total_cache_creation_tokens,
        total_cache_read_tokens: r.total_cache_read_tokens,
    }
}

// ── MCP servers ────────────────────────────────────────────────────────────────

fn mcp_to_proto(m: &StoredMcpServer) -> sp::McpServer {
    sp::McpServer {
        name: m.name.clone(),
        url: m.url.clone(),
        headers: m
            .headers
            .iter()
            .map(|(name, value)| sp::McpHeader {
                name: name.clone(),
                value: value.clone(),
                __buffa_unknown_fields: Default::default(),
            })
            .collect(),
        __buffa_unknown_fields: Default::default(),
    }
}

fn mcp_from_proto(m: &sp::McpServer) -> StoredMcpServer {
    StoredMcpServer {
        name: m.name.clone(),
        url: m.url.clone(),
        headers: m.headers.iter().map(|h| (h.name.clone(), h.value.clone())).collect(),
    }
}

// ── Tool policies ──────────────────────────────────────────────────────────────

fn tool_policy_to_proto(p: &ToolPolicy) -> sp::ToolPolicy {
    let action = match p.action {
        PolicyAction::Allow => sp::PolicyAction::POLICY_ACTION_ALLOW,
        PolicyAction::RequireApproval => sp::PolicyAction::POLICY_ACTION_REQUIRE_APPROVAL,
        PolicyAction::Deny => sp::PolicyAction::POLICY_ACTION_DENY,
    };
    sp::ToolPolicy {
        tool: p.tool.clone(),
        path_pattern: p.path_pattern.clone(),
        action: action.into(),
        __buffa_unknown_fields: Default::default(),
    }
}

fn tool_policy_from_proto(p: &sp::ToolPolicy) -> ToolPolicy {
    let action = match p.action.as_known() {
        Some(sp::PolicyAction::POLICY_ACTION_REQUIRE_APPROVAL) => PolicyAction::RequireApproval,
        Some(sp::PolicyAction::POLICY_ACTION_DENY) => PolicyAction::Deny,
        _ => PolicyAction::Allow,
    };
    ToolPolicy {
        tool: p.tool.clone(),
        path_pattern: p.path_pattern.clone(),
        action,
    }
}

// ── Audit log ──────────────────────────────────────────────────────────────────

fn audit_to_proto(a: &AuditEntry) -> sp::AuditEntry {
    let outcome = match a.outcome {
        AuditOutcome::Allowed => sp::AuditOutcome::AUDIT_OUTCOME_ALLOWED,
        AuditOutcome::Denied => sp::AuditOutcome::AUDIT_OUTCOME_DENIED,
        AuditOutcome::RequiredApproval => sp::AuditOutcome::AUDIT_OUTCOME_REQUIRED_APPROVAL,
        AuditOutcome::ApprovedByUser => sp::AuditOutcome::AUDIT_OUTCOME_APPROVED_BY_USER,
        AuditOutcome::DeniedByUser => sp::AuditOutcome::AUDIT_OUTCOME_DENIED_BY_USER,
    };
    sp::AuditEntry {
        timestamp: a.timestamp.clone(),
        tool: a.tool.clone(),
        input_summary: a.input_summary.clone(),
        outcome: outcome.into(),
        __buffa_unknown_fields: Default::default(),
    }
}

fn audit_from_proto(a: &sp::AuditEntry) -> AuditEntry {
    let outcome = match a.outcome.as_known() {
        Some(sp::AuditOutcome::AUDIT_OUTCOME_DENIED) => AuditOutcome::Denied,
        Some(sp::AuditOutcome::AUDIT_OUTCOME_REQUIRED_APPROVAL) => AuditOutcome::RequiredApproval,
        Some(sp::AuditOutcome::AUDIT_OUTCOME_APPROVED_BY_USER) => AuditOutcome::ApprovedByUser,
        Some(sp::AuditOutcome::AUDIT_OUTCOME_DENIED_BY_USER) => AuditOutcome::DeniedByUser,
        _ => AuditOutcome::Allowed,
    };
    AuditEntry {
        timestamp: a.timestamp.clone(),
        tool: a.tool.clone(),
        input_summary: a.input_summary.clone(),
        outcome,
    }
}

// ── Todos ──────────────────────────────────────────────────────────────────────

fn todo_to_proto(t: &TodoItem) -> sp::TodoItem {
    sp::TodoItem {
        id: t.id.clone(),
        content: t.content.clone(),
        status: t.status.clone(),
        __buffa_unknown_fields: Default::default(),
    }
}

fn todo_from_proto(t: &sp::TodoItem) -> TodoItem {
    TodoItem {
        id: t.id.clone(),
        content: t.content.clone(),
        status: t.status.clone(),
    }
}

// ── Egress policy ──────────────────────────────────────────────────────────────

fn egress_action_to_proto(a: &EgressAction) -> sp::EgressAction {
    match a {
        EgressAction::Allow => sp::EgressAction::EGRESS_ACTION_ALLOW,
        EgressAction::Deny => sp::EgressAction::EGRESS_ACTION_DENY,
    }
}

fn egress_action_from_proto(v: &buffa::EnumValue<sp::EgressAction>) -> EgressAction {
    match v.as_known() {
        Some(sp::EgressAction::EGRESS_ACTION_ALLOW) => EgressAction::Allow,
        _ => EgressAction::Deny,
    }
}

fn egress_to_proto(p: &EgressPolicy) -> sp::EgressPolicy {
    sp::EgressPolicy {
        default_action: egress_action_to_proto(&p.default_action).into(),
        rules: p
            .rules
            .iter()
            .map(|r| sp::EgressRule {
                host_pattern: r.host_pattern.clone(),
                action: egress_action_to_proto(&r.action).into(),
                __buffa_unknown_fields: Default::default(),
            })
            .collect(),
        __buffa_unknown_fields: Default::default(),
    }
}

fn egress_from_proto(p: &sp::EgressPolicy) -> EgressPolicy {
    EgressPolicy {
        default_action: egress_action_from_proto(&p.default_action),
        rules: p
            .rules
            .iter()
            .map(|r| EgressRule {
                host_pattern: r.host_pattern.clone(),
                action: egress_action_from_proto(&r.action),
            })
            .collect(),
    }
}

// ── Messages (mirror trogon-compactor/src/wire.rs) ─────────────────────────────

fn message_to_proto(m: &Message) -> cp::Message {
    cp::Message {
        role: m.role.clone(),
        content: m.content.iter().map(block_to_proto).collect(),
        __buffa_unknown_fields: Default::default(),
    }
}

fn block_to_proto(b: &ContentBlock) -> cp::ContentBlock {
    let kind = match b {
        ContentBlock::Text { text } => BlockKind::Text(Box::new(cp::TextBlock {
            text: text.clone(),
            __buffa_unknown_fields: Default::default(),
        })),
        ContentBlock::Image { source } => BlockKind::Image(Box::new(cp::ImageBlock {
            source_json: serde_json::to_string(source).unwrap_or_default(),
            __buffa_unknown_fields: Default::default(),
        })),
        ContentBlock::Thinking { thinking } => BlockKind::Thinking(Box::new(cp::ThinkingBlock {
            thinking: thinking.clone(),
            __buffa_unknown_fields: Default::default(),
        })),
        ContentBlock::ToolUse {
            id,
            name,
            input,
            parent_tool_use_id,
        } => BlockKind::ToolUse(Box::new(cp::ToolUseBlock {
            id: id.clone(),
            name: name.clone(),
            input_json: input.to_string(),
            parent_tool_use_id: parent_tool_use_id.clone(),
            __buffa_unknown_fields: Default::default(),
        })),
        ContentBlock::ToolResult { tool_use_id, content } => BlockKind::ToolResult(Box::new(cp::ToolResultBlock {
            tool_use_id: tool_use_id.clone(),
            content: content.clone(),
            __buffa_unknown_fields: Default::default(),
        })),
    };
    cp::ContentBlock {
        kind: Some(kind),
        __buffa_unknown_fields: Default::default(),
    }
}

fn proto_to_message(m: &cp::Message) -> Option<Message> {
    Some(Message {
        role: m.role.clone(),
        content: m.content.iter().filter_map(proto_to_block).collect(),
    })
}

fn proto_to_block(b: &cp::ContentBlock) -> Option<ContentBlock> {
    match b.kind.as_ref()? {
        BlockKind::Text(t) => Some(ContentBlock::Text { text: t.text.clone() }),
        BlockKind::Image(i) => {
            let source: ImageSource = serde_json::from_str(&i.source_json).ok()?;
            Some(ContentBlock::Image { source })
        }
        BlockKind::Thinking(t) => Some(ContentBlock::Thinking {
            thinking: t.thinking.clone(),
        }),
        BlockKind::ToolUse(t) => Some(ContentBlock::ToolUse {
            id: t.id.clone(),
            name: t.name.clone(),
            input: serde_json::from_str(&t.input_json).unwrap_or(serde_json::Value::Null),
            parent_tool_use_id: t.parent_tool_use_id.clone(),
        }),
        BlockKind::ToolResult(t) => Some(ContentBlock::ToolResult {
            tool_use_id: t.tool_use_id.clone(),
            content: t.content.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_store::{AuditEntry, ToolPolicy};

    /// A `SessionState` with every field populated to a non-default value,
    /// including every nested type and every enum variant.
    fn fully_populated() -> SessionState {
        SessionState {
            messages: vec![Message {
                role: "assistant".into(),
                content: vec![
                    ContentBlock::Text { text: "hi".into() },
                    ContentBlock::Image {
                        source: ImageSource::Base64 {
                            media_type: "image/png".into(),
                            data: "abc".into(),
                        },
                    },
                    ContentBlock::Thinking { thinking: "hmm".into() },
                    ContentBlock::ToolUse {
                        id: "t1".into(),
                        name: "Read".into(),
                        input: serde_json::json!({"path": "/x"}),
                        parent_tool_use_id: Some("p1".into()),
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: "t1".into(),
                        content: "ok".into(),
                    },
                ],
            }],
            model: Some("claude-opus-4-6".into()),
            compactor_provider: Some("anthropic".into()),
            compactor_model: Some("claude-haiku-4-5".into()),
            mode: "acceptEdits".into(),
            cwd: "/work".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-02T00:00:00Z".into(),
            title: "session title".into(),
            mcp_servers: vec![StoredMcpServer {
                name: "srv".into(),
                url: "https://mcp.example/sse".into(),
                headers: vec![("Authorization".into(), "Bearer x".into())],
            }],
            system_prompt: Some("be helpful".into()),
            additional_roots: vec!["/root-a".into(), "/root-b".into()],
            disable_builtin_tools: true,
            allowed_tools: vec!["Read".into(), "Write".into()],
            parent_session_id: Some("root".into()),
            branched_at_index: Some(7),
            tool_policies: vec![
                ToolPolicy {
                    tool: "Write".into(),
                    path_pattern: "/work/**".into(),
                    action: PolicyAction::Allow,
                },
                ToolPolicy {
                    tool: "Bash".into(),
                    path_pattern: "**".into(),
                    action: PolicyAction::RequireApproval,
                },
                ToolPolicy {
                    tool: "Delete".into(),
                    path_pattern: "/etc/**".into(),
                    action: PolicyAction::Deny,
                },
            ],
            egress_policy: Some(EgressPolicy {
                default_action: EgressAction::Deny,
                rules: vec![EgressRule {
                    host_pattern: "api.anthropic.com".into(),
                    action: EgressAction::Allow,
                }],
            }),
            audit_log: vec![
                AuditEntry {
                    timestamp: "2026-01-01T00:00:00Z".into(),
                    tool: "Read".into(),
                    input_summary: "/x".into(),
                    outcome: AuditOutcome::Allowed,
                },
                AuditEntry {
                    timestamp: "2026-01-01T00:00:01Z".into(),
                    tool: "Bash".into(),
                    input_summary: "rm".into(),
                    outcome: AuditOutcome::Denied,
                },
                AuditEntry {
                    timestamp: "2026-01-01T00:00:02Z".into(),
                    tool: "Write".into(),
                    input_summary: "f".into(),
                    outcome: AuditOutcome::RequiredApproval,
                },
                AuditEntry {
                    timestamp: "2026-01-01T00:00:03Z".into(),
                    tool: "Write".into(),
                    input_summary: "g".into(),
                    outcome: AuditOutcome::ApprovedByUser,
                },
                AuditEntry {
                    timestamp: "2026-01-01T00:00:04Z".into(),
                    tool: "Write".into(),
                    input_summary: "h".into(),
                    outcome: AuditOutcome::DeniedByUser,
                },
            ],
            terminal_id: Some("term-1".into()),
            terminal_cwd: Some("/work".into()),
            todos: vec![TodoItem {
                id: "td1".into(),
                content: "do it".into(),
                status: "pending".into(),
            }],
            permission_rules_text: Some("## Permissions\nallow Read".into()),
            spawn_depth: 2,
            token_budget: 123_456,
            total_input_tokens: 1000,
            total_output_tokens: 500,
            total_cache_creation_tokens: 200,
            total_cache_read_tokens: 75,
        }
    }

    #[test]
    fn round_trip_preserves_every_field() {
        let original = fully_populated();
        let bytes = encode(&original);
        let back = decode(&bytes).expect("decode");
        // Compare via JSON: any lost/altered field surfaces as a diff (both derive Serialize).
        assert_eq!(
            serde_json::to_value(&original).unwrap(),
            serde_json::to_value(&back).unwrap(),
        );
    }

    #[test]
    fn default_state_round_trips() {
        let original = SessionState::default();
        let bytes = encode(&original);
        let back = decode(&bytes).expect("decode");
        assert_eq!(
            serde_json::to_value(&original).unwrap(),
            serde_json::to_value(&back).unwrap(),
        );
    }

    #[test]
    fn decode_rejects_non_protobuf_garbage() {
        // Old serde JSON or random bytes must not silently produce a bogus session.
        let invalid = b"\xff\xff not protobuf \x00\x01\x02 random";
        assert!(decode(invalid).is_err());
    }

    #[test]
    fn compaction_override_round_trips() {
        let bytes = encode_compaction(Some("anthropic"), Some("claude-haiku-4-5"));
        assert_eq!(
            decode_compaction(&bytes),
            (Some("anthropic".to_string()), Some("claude-haiku-4-5".to_string()))
        );
    }

    #[test]
    fn compaction_override_none_round_trips() {
        let bytes = encode_compaction(None, None);
        assert_eq!(decode_compaction(&bytes), (None, None));
    }
}
