use trogonai_session_contracts::{ContentBlock, ProjectionBlockKind, __buffa::oneof::content_block::Kind as BlockKind};

/// Token budget policy for prompt projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenBudget {
    pub max_input_tokens: u64,
    pub output_reserve_tokens: u64,
}

impl TokenBudget {
    pub fn available_input_tokens(&self) -> u64 {
        self.max_input_tokens.saturating_sub(self.output_reserve_tokens)
    }

    pub fn from_model_window(max_context_tokens: u64, output_reserve_tokens: u64) -> Self {
        Self {
            max_input_tokens: max_context_tokens,
            output_reserve_tokens,
        }
    }
}

/// Deterministic token estimate for plain text (approx 4 chars per token).
pub fn estimate_text_tokens(text: &str) -> u64 {
    let char_count = text.chars().count() as u64;
    char_count.div_ceil(4).max(1)
}

/// Deterministic token estimate for content blocks after projection transforms.
pub fn estimate_content_tokens(blocks: &[ContentBlock]) -> u64 {
    blocks
        .iter()
        .map(estimate_block_tokens)
        .sum::<u64>()
        .max(1)
}

fn estimate_block_tokens(block: &ContentBlock) -> u64 {
    match block.kind.as_ref() {
        Some(BlockKind::Text(text)) => estimate_text_tokens(text),
        Some(BlockKind::ImageRef(artifact)) => estimate_text_tokens(&artifact.preview),
        Some(BlockKind::Thinking(thinking)) => estimate_text_tokens(thinking),
        Some(BlockKind::ToolUse(tool_use)) => {
            estimate_text_tokens(&tool_use.name) + estimate_text_tokens(&tool_use.input_json) + 8
        }
        Some(BlockKind::ToolResult(tool_result)) => {
            estimate_text_tokens(&tool_result.tool_use_id)
                + tool_result
                    .result
                    .as_option()
                    .map(|result| match result.kind.as_ref() {
                        Some(trogonai_session_contracts::__buffa::oneof::tool_call_result::Kind::Text(text)) => {
                            estimate_text_tokens(&text.content)
                        }
                        Some(trogonai_session_contracts::__buffa::oneof::tool_call_result::Kind::ArtifactRef(
                            artifact,
                        )) => estimate_text_tokens(&artifact.preview),
                        None => 1,
                    })
                    .unwrap_or(1)
        }
        None => 1,
    }
}

/// Stable priority ordering from the token budget policy in cambio-modelo.md.
pub fn block_priority(kind: ProjectionBlockKind) -> u8 {
    match kind {
        ProjectionBlockKind::SystemRules => 1,
        ProjectionBlockKind::SafetyPermissions => 2,
        ProjectionBlockKind::ContextTwin => 3,
        ProjectionBlockKind::SwitchAdaptation => 4,
        ProjectionBlockKind::ContinuityWarning => 5,
        ProjectionBlockKind::CurrentRequest => 6,
        ProjectionBlockKind::RecentTurn => 7,
        ProjectionBlockKind::ToolSchema => 8,
        ProjectionBlockKind::ArtifactPreview => 9,
        ProjectionBlockKind::UnresolvedError => 10,
        ProjectionBlockKind::Summary => 11,
        ProjectionBlockKind::OlderTranscript => 12,
        ProjectionBlockKind::Unspecified => 255,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_budget_reserves_output_tokens() {
        let budget = TokenBudget::from_model_window(10_000, 1_000);
        assert_eq!(budget.available_input_tokens(), 9_000);
    }

    #[test]
    fn text_token_estimate_is_deterministic() {
        assert_eq!(estimate_text_tokens("abcd"), 1);
        assert_eq!(estimate_text_tokens("abcdefgh"), 2);
    }
}
