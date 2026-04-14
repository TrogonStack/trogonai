//! Compaction detection: decides *whether* and *where* to compact.

use crate::tokens::{estimate_message_tokens, estimate_total_tokens};
use crate::types::Message;

/// Thresholds that control compaction behaviour.
#[derive(Debug, Clone)]
pub struct CompactionSettings {
    /// Total context window of the target model in tokens (default: 200 000).
    pub context_window: usize,
    /// Tokens to reserve for new output and incoming tool results (default: 16 384).
    pub reserve_tokens: usize,
    /// Minimum number of recent tokens to keep verbatim after compaction (default: 20 000).
    pub keep_recent_tokens: usize,
}

impl Default for CompactionSettings {
    fn default() -> Self {
        Self {
            context_window: 200_000,
            reserve_tokens: 16_384,
            keep_recent_tokens: 20_000,
        }
    }
}

/// Returns `true` when the conversation is too close to the context window limit.
pub fn should_compact(messages: &[Message], settings: &CompactionSettings) -> bool {
    let tokens = estimate_total_tokens(messages);
    tokens > settings.context_window.saturating_sub(settings.reserve_tokens)
}

/// Returns the index of the first message to **keep** verbatim.
///
/// All messages before this index will be summarized.  The cut point is always
/// placed at a real user turn (not a tool-result message) so we never split a
/// tool-use / tool-result pair.
///
/// Returns `None` if the conversation is already small enough that nothing
/// useful can be cut.
pub fn find_cut_point(messages: &[Message], keep_recent_tokens: usize) -> Option<usize> {
    let mut accumulated: usize = 0;

    // Walk backwards, accumulating token counts until we've covered
    // keep_recent_tokens worth of history.
    for i in (0..messages.len()).rev() {
        accumulated += estimate_message_tokens(&messages[i]);

        if accumulated >= keep_recent_tokens {
            // Scan *backward* from i to find the nearest preceding valid cut
            // point: a user message that is NOT a pure tool-result message.
            // Scanning backward means we may keep slightly more than
            // keep_recent_tokens, which is acceptable (it's a minimum, not a
            // hard cap).
            for j in (0..=i).rev() {
                if messages[j].role == "user" && !messages[j].is_tool_result_only() {
                    // Cutting at index 0 means there's nothing to summarize.
                    if j == 0 {
                        return None;
                    }
                    return Some(j);
                }
            }
            // No valid boundary found — nothing actionable.
            return None;
        }
    }

    // Everything fits within keep_recent_tokens; nothing to compact.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ContentBlock, Message};

    fn turn(role: &str, chars: usize) -> Message {
        Message {
            role: role.into(),
            content: vec![ContentBlock::Text { text: "x".repeat(chars) }],
        }
    }

    fn tool_result_msg() -> Message {
        Message {
            role: "user".into(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "id".into(),
                content: "result".into(),
            }],
        }
    }

    #[test]
    fn should_compact_false_when_well_under_limit() {
        let settings = CompactionSettings {
            context_window: 200_000,
            reserve_tokens: 16_384,
            keep_recent_tokens: 20_000,
        };
        // 10 messages × 4 chars → ~10 total tokens, nowhere near the limit
        let msgs: Vec<_> = (0..10).map(|_| turn("user", 4)).collect();
        assert!(!should_compact(&msgs, &settings));
    }

    #[test]
    fn should_compact_true_when_over_limit() {
        let settings = CompactionSettings {
            context_window: 100,
            reserve_tokens: 20,
            keep_recent_tokens: 30,
        };
        // 40 messages × 16 chars → ~160 tokens, over 80-token threshold
        let msgs: Vec<_> = (0..40)
            .flat_map(|_| [turn("user", 16), turn("assistant", 16)])
            .collect();
        assert!(should_compact(&msgs, &settings));
    }

    #[test]
    fn cut_point_lands_on_user_turn() {
        let msgs: Vec<_> = (0..20)
            .flat_map(|i| {
                [
                    turn("user", 50 * (i + 1)),
                    turn("assistant", 50 * (i + 1)),
                ]
            })
            .collect();

        if let Some(idx) = find_cut_point(&msgs, 300) {
            assert_eq!(msgs[idx].role, "user");
            assert!(!msgs[idx].is_tool_result_only());
        }
    }

    #[test]
    fn cut_point_skips_pure_tool_result_messages() {
        // Alternate: user-text, assistant, tool-result-user, user-text, ...
        let msgs = vec![
            turn("user", 200),
            turn("assistant", 200),
            tool_result_msg(),
            turn("user", 200),   // ← valid cut point
            turn("assistant", 200),
            tool_result_msg(),
            turn("user", 50),    // kept verbatim
        ];
        let cut = find_cut_point(&msgs, 120);
        if let Some(idx) = cut {
            assert!(
                !msgs[idx].is_tool_result_only(),
                "cut landed on a tool-result message"
            );
        }
    }

    #[test]
    fn cut_point_none_when_everything_fits() {
        let settings_keep = 10_000;
        // Only 2 tiny messages — well under keep_recent_tokens
        let msgs = vec![turn("user", 4), turn("assistant", 4)];
        assert!(find_cut_point(&msgs, settings_keep).is_none());
    }

    #[test]
    fn should_compact_empty_messages_is_false() {
        let settings = CompactionSettings::default();
        assert!(!should_compact(&[], &settings));
    }

    #[test]
    fn find_cut_point_empty_messages_returns_none() {
        assert!(find_cut_point(&[], 1_000).is_none());
    }

    #[test]
    fn find_cut_point_none_when_only_valid_cut_is_at_index_zero() {
        // [0] real user turn — the only valid cut point, but j==0 → None
        // [1] assistant (big, triggers keep_recent accumulation)
        // [2] tool-result user (big, not a valid cut)
        let msgs = vec![
            turn("user", 400),
            turn("assistant", 400),
            tool_result_msg(),
        ];
        // keep_recent_tokens = 10 → threshold hit at i=2 or i=1
        // backward scan only finds a valid user turn at index 0 → None
        let result = find_cut_point(&msgs, 10);
        assert!(result.is_none());
    }

    #[test]
    fn should_compact_when_reserve_exceeds_context_window() {
        // saturating_sub clamps to 0 → threshold is 0 → any non-empty message triggers compaction
        let settings = CompactionSettings {
            context_window: 100,
            reserve_tokens: 200, // larger than context_window
            keep_recent_tokens: 20,
        };
        let msgs = vec![turn("user", 1)]; // even 1 char → 1 token > 0
        assert!(should_compact(&msgs, &settings));
    }

    #[test]
    fn find_cut_point_none_when_all_user_messages_are_tool_results() {
        // No real user turns at all → backward scan finds nothing → None
        let msgs = vec![
            tool_result_msg(),
            turn("assistant", 200),
            tool_result_msg(),
            turn("assistant", 200),
            tool_result_msg(),
        ];
        assert!(find_cut_point(&msgs, 10).is_none());
    }
}
