//! Token estimation.
//!
//! We use the `chars / 4` heuristic (same as pi-mono's reference implementation).
//! It intentionally overestimates to stay conservative: it's better to compact
//! slightly too early than to hit the hard API limit.

use crate::types::{ContentBlock, Message};

/// Rough token estimate for an arbitrary string: 1 token ≈ 4 UTF-8 bytes.
pub fn estimate_tokens(text: &str) -> usize {
    // Equivalent to ceil(len / 4)
    text.len().div_ceil(4)
}

/// Estimates the token cost of a single message.
pub fn estimate_message_tokens(msg: &Message) -> usize {
    let content: usize = msg.content.iter().map(estimate_block_tokens).sum();
    // +4 overhead for role field and structural framing per Anthropic message
    content + 4
}

fn estimate_block_tokens(block: &ContentBlock) -> usize {
    match block.as_text() {
        Some(t) => estimate_tokens(t),
        // Images are typically ~300 tokens in vision models
        None => 300,
    }
}

/// Total token estimate across a slice of messages.
pub fn estimate_total_tokens(messages: &[Message]) -> usize {
    messages.iter().map(estimate_message_tokens).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_tokens_rounds_up() {
        assert_eq!(estimate_tokens("abcd"), 1); // exactly 4 chars → 1 token
        assert_eq!(estimate_tokens("abcde"), 2); // 5 chars → 2 tokens (ceil)
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn estimate_message_tokens_includes_overhead() {
        let msg = Message::user("abcd"); // 1 token + 4 overhead = 5
        assert_eq!(estimate_message_tokens(&msg), 5);
    }

    #[test]
    fn estimate_total_is_sum_of_messages() {
        let msgs = vec![Message::user("abcd"), Message::assistant("abcd")];
        // Each message: 1 content token + 4 overhead = 5, total = 10
        assert_eq!(estimate_total_tokens(&msgs), 10);
    }

    #[test]
    fn image_block_estimates_300_tokens() {
        use crate::types::ContentBlock;
        let msg = crate::types::Message {
            role: "user".into(),
            content: vec![ContentBlock::Image {
                source: serde_json::json!({}),
            }],
        };
        // image = 300 (hardcoded) + 4 overhead = 304
        assert_eq!(estimate_message_tokens(&msg), 304);
    }

    #[test]
    fn estimate_tokens_counts_bytes_not_chars_for_multibyte() {
        // Each emoji is 4 bytes in UTF-8 → 1 token per emoji
        assert_eq!(estimate_tokens("😀"), 1);
        // Two emojis = 8 bytes → 2 tokens
        assert_eq!(estimate_tokens("😀😀"), 2);
        // CJK character: 3 bytes → ceil(3/4) = 1 token
        assert_eq!(estimate_tokens("中"), 1);
        // Four CJK: 12 bytes → 3 tokens
        assert_eq!(estimate_tokens("中中中中"), 3);
    }
}
