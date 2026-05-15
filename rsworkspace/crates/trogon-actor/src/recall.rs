use trogon_transcript::entry::{Role, TranscriptEntry};

/// Maximum number of message entries included in the formatted history block.
///
/// Tool calls, routing decisions, and sub-agent spawns are skipped — they add
/// noise without useful context for the next LLM invocation.
const MAX_MESSAGES: usize = 100;

/// Maximum character length of a single message included in the history.
/// Longer content is truncated and marked with `…`.
const MAX_CONTENT_CHARS: usize = 500;

/// Format past transcript entries into a text block an actor can prepend to its
/// LLM prompt.
///
/// Returns `None` when there are no `Message` entries in `entries` (e.g. the
/// very first invocation of a new entity, or a history that contains only tool
/// calls and routing decisions).
///
/// Only [`TranscriptEntry::Message`] variants are included. The most recent
/// [`MAX_MESSAGES`] messages are kept when the total exceeds the cap.
pub fn format_history(entries: &[TranscriptEntry]) -> Option<String> {
    let messages: Vec<(&Role, &str, u64)> = entries
        .iter()
        .filter_map(|e| match e {
            TranscriptEntry::Message {
                role,
                content,
                timestamp,
                ..
            } => Some((role, content.as_str(), *timestamp)),
            _ => None,
        })
        .collect();

    if messages.is_empty() {
        return None;
    }

    let total = messages.len();
    let window = if total > MAX_MESSAGES {
        &messages[total - MAX_MESSAGES..]
    } else {
        &messages[..]
    };

    let lines: Vec<String> = window
        .iter()
        .map(|(role, content, ts)| {
            let label = match role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::System => "system",
                Role::Tool => "tool",
            };
            let ts_str = format_timestamp_ms(*ts);
            let body = truncate(content, MAX_CONTENT_CHARS);
            format!("[{ts_str}] {label}: {body}")
        })
        .collect();

    let shown = window.len();
    let header = if total > MAX_MESSAGES {
        format!(
            "## Entity history ({total} messages total, showing last {shown})\n\n"
        )
    } else {
        format!("## Entity history ({total} messages)\n\n")
    };

    Some(format!("{header}{}", lines.join("\n")))
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max_chars - 1).collect();
        t.push('…');
        t
    }
}

/// Format a Unix timestamp in milliseconds as `YYYY-MM-DD HH:MMZ`.
///
/// Pure arithmetic — no external time crates required.
fn format_timestamp_ms(ms: u64) -> String {
    let secs = ms / 1000;
    let time_of_day = secs % 86400;
    let h = time_of_day / 3600;
    let m = (time_of_day % 3600) / 60;

    // Days since 1970-01-01 → Gregorian calendar via the Fliegel–Van Flandern algorithm
    let z = secs / 86400 + 719468;
    let era = z / 146097;
    let doe = z % 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + u64::from(mo <= 2);

    format!("{year:04}-{mo:02}-{d:02} {h:02}:{m:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_transcript::entry::TranscriptEntry;

    fn msg(role: Role, content: &str, ts: u64) -> TranscriptEntry {
        TranscriptEntry::Message {
            role,
            content: content.to_string(),
            timestamp: ts,
            tokens: None,
        }
    }

    #[test]
    fn empty_entries_returns_none() {
        assert!(format_history(&[]).is_none());
    }

    #[test]
    fn only_tool_calls_returns_none() {
        let entries = vec![TranscriptEntry::ToolCall {
            name: "search".into(),
            input: serde_json::json!({}),
            output: serde_json::json!({}),
            duration_ms: 10,
            timestamp: 1_000_000,
        }];
        assert!(format_history(&entries).is_none());
    }

    #[test]
    fn single_message_is_formatted() {
        let entries = vec![msg(Role::User, "review this PR", 1_747_000_000_000)];
        let out = format_history(&entries).unwrap();
        assert!(out.contains("user: review this PR"));
        assert!(out.contains("## Entity history (1 messages)"));
    }

    #[test]
    fn tool_calls_are_skipped() {
        let entries = vec![
            msg(Role::User, "hello", 1_000),
            TranscriptEntry::ToolCall {
                name: "x".into(),
                input: serde_json::json!({}),
                output: serde_json::json!({}),
                duration_ms: 5,
                timestamp: 2_000,
            },
            msg(Role::Assistant, "world", 3_000),
        ];
        let out = format_history(&entries).unwrap();
        assert!(out.contains("user: hello"));
        assert!(out.contains("assistant: world"));
        assert!(!out.contains("tool_call"));
        // 2 messages total
        assert!(out.contains("(2 messages)"));
    }

    #[test]
    fn long_content_is_truncated() {
        let long = "x".repeat(600);
        let entries = vec![msg(Role::User, &long, 1_000)];
        let out = format_history(&entries).unwrap();
        assert!(out.contains('…'));
        // The formatted line must not exceed MAX_CONTENT_CHARS + some overhead
        let line = out.lines().find(|l| l.contains("user:")).unwrap();
        let content_part = line.splitn(2, "user: ").nth(1).unwrap();
        assert!(content_part.chars().count() <= MAX_CONTENT_CHARS);
    }

    #[test]
    fn cap_keeps_latest_messages() {
        // 105 messages: first is labelled "msg-0", last is "msg-104"
        let entries: Vec<TranscriptEntry> = (0..105u64)
            .map(|i| msg(Role::User, &format!("msg-{i}"), i * 1000))
            .collect();
        let out = format_history(&entries).unwrap();
        // Header mentions total = 105, shown = 100
        assert!(out.contains("105 messages total, showing last 100"));
        // Oldest 5 (msg-0 .. msg-4) are NOT present. Use "user: msg-N\n" to
        // avoid false matches: e.g. "msg-4" is a substring of "msg-40".
        assert!(!out.contains("user: msg-0\n"));
        assert!(!out.contains("user: msg-4\n"));
        // Most recent (msg-104) IS present
        assert!(out.contains("msg-104"));
        assert!(out.contains("msg-5"));
    }

    #[test]
    fn timestamp_formatting_known_date() {
        // 2026-05-14 12:30 UTC = 1_778_761_800 seconds → ms
        let ms: u64 = 1_778_761_800 * 1000;
        let entries = vec![msg(Role::User, "test", ms)];
        let out = format_history(&entries).unwrap();
        assert!(out.contains("2026-05-14 12:30Z"), "got: {out}");
    }

    #[test]
    fn sub_agent_spawn_and_routing_are_skipped() {
        let entries = vec![
            TranscriptEntry::SubAgentSpawn {
                parent: "pr/repo/1".into(),
                child: "SecurityActor".into(),
                capability: "security_analysis".into(),
                timestamp: 1_000,
            },
            TranscriptEntry::RoutingDecision {
                from: "gateway".into(),
                to: "PrActor".into(),
                reasoning: "it's a PR event".into(),
                timestamp: 2_000,
            },
            msg(Role::Assistant, "done", 3_000),
        ];
        let out = format_history(&entries).unwrap();
        assert!(out.contains("assistant: done"));
        assert!(out.contains("(1 messages)"));
    }
}
