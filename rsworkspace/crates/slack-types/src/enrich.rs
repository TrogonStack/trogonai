use std::collections::{HashMap, HashSet};

use crate::events::SessionType;

pub fn compute_session_key(
    session_type: &SessionType,
    channel_id: &str,
    user_id: &str,
    thread_ts: Option<&str>,
) -> Option<String> {
    let key = match session_type {
        SessionType::Direct => format!("slack:dm:{}", user_id),
        SessionType::Channel => match thread_ts {
            Some(tts) => format!("slack:channel:{}:thread:{}", channel_id, tts),
            None => format!("slack:channel:{}", channel_id),
        },
        SessionType::Group => match thread_ts {
            Some(tts) => format!("slack:group:{}:thread:{}", channel_id, tts),
            None => format!("slack:group:{}", channel_id),
        },
    };
    Some(key)
}

pub fn strip_bot_mention(text: &str, bot_user_id: Option<&str>) -> String {
    match bot_user_id {
        Some(uid) => text.replace(&format!("<@{}>", uid), "").trim().to_string(),
        None => text.trim().to_string(),
    }
}

/// Returns `true` when mention gating is active (message should be dropped
/// unless it contains an @mention of the bot).
pub fn resolve_mention_gating(
    session_type: &SessionType,
    channel_id: &str,
    thread_ts: Option<&str>,
    mention_gating: bool,
    mention_gating_channels: &HashSet<String>,
    no_mention_channels: &HashSet<String>,
    text: &str,
    mention_patterns: &[String],
) -> bool {
    if matches!(session_type, SessionType::Direct) || thread_ts.is_some() {
        return false;
    }
    if !mention_patterns.is_empty() {
        let text_lower = text.to_lowercase();
        if mention_patterns
            .iter()
            .any(|p| text_lower.contains(&p.to_lowercase()))
        {
            return false;
        }
    }
    if no_mention_channels.contains(channel_id) {
        return false;
    }
    if mention_gating_channels.contains(channel_id) {
        return true;
    }
    mention_gating
}

/// Replace `:name:` emoji shortcodes from the workspace's custom emoji list
/// with `:name: [custom emoji]` so an LLM can identify them.
pub fn annotate_custom_emoji(text: &str, custom_emoji: &HashMap<String, String>) -> String {
    if custom_emoji.is_empty() || !text.contains(':') {
        return text.to_string();
    }
    let mut result = text.to_string();
    for name in custom_emoji.keys() {
        let pattern = format!(":{name}:");
        if result.contains(&pattern) {
            result = result.replace(&pattern, &format!(":{name}: [custom emoji]"));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── strip_bot_mention ───────────────────────────────────────────────────

    #[test]
    fn strip_removes_known_mention() {
        assert_eq!(
            strip_bot_mention("<@UBOT123> hello", Some("UBOT123")),
            "hello"
        );
    }

    #[test]
    fn strip_removes_mention_mid_text() {
        assert_eq!(
            strip_bot_mention("hey <@UBOT123> what's up", Some("UBOT123")),
            "hey  what's up"
        );
    }

    #[test]
    fn strip_no_bot_id_only_trims() {
        assert_eq!(strip_bot_mention("  hello  ", None), "hello");
    }

    #[test]
    fn strip_unknown_bot_id_leaves_text() {
        assert_eq!(
            strip_bot_mention("<@UOTHER> hello", Some("UBOT123")),
            "<@UOTHER> hello"
        );
    }

    #[test]
    fn strip_empty_text() {
        assert_eq!(strip_bot_mention("", Some("UBOT123")), "");
    }

    // ── compute_session_key ─────────────────────────────────────────────────

    #[test]
    fn session_key_dm() {
        assert_eq!(
            compute_session_key(&SessionType::Direct, "C1", "U1", None),
            Some("slack:dm:U1".to_string())
        );
    }

    #[test]
    fn session_key_channel_no_thread() {
        assert_eq!(
            compute_session_key(&SessionType::Channel, "C1", "U1", None),
            Some("slack:channel:C1".to_string())
        );
    }

    #[test]
    fn session_key_channel_with_thread() {
        assert_eq!(
            compute_session_key(&SessionType::Channel, "C1", "U1", Some("1234.5")),
            Some("slack:channel:C1:thread:1234.5".to_string())
        );
    }

    #[test]
    fn session_key_group_with_thread() {
        assert_eq!(
            compute_session_key(&SessionType::Group, "G1", "U1", Some("9.0")),
            Some("slack:group:G1:thread:9.0".to_string())
        );
    }

    // ── resolve_mention_gating ──────────────────────────────────────────────

    fn make_sets(channels: &[&str]) -> HashSet<String> {
        channels.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn gating_dm_always_off() {
        assert!(!resolve_mention_gating(
            &SessionType::Direct,
            "C1",
            None,
            true,
            &make_sets(&["C1"]),
            &HashSet::new(),
            "",
            &[]
        ));
    }

    #[test]
    fn gating_thread_reply_always_off() {
        assert!(!resolve_mention_gating(
            &SessionType::Channel,
            "C1",
            Some("123.0"),
            true,
            &HashSet::new(),
            &HashSet::new(),
            "",
            &[]
        ));
    }

    #[test]
    fn gating_no_mention_channel_overrides_global_on() {
        assert!(!resolve_mention_gating(
            &SessionType::Channel,
            "C1",
            None,
            true,
            &HashSet::new(),
            &make_sets(&["C1"]),
            "",
            &[]
        ));
    }

    #[test]
    fn gating_mention_gating_channel_overrides_global_off() {
        assert!(resolve_mention_gating(
            &SessionType::Channel,
            "C1",
            None,
            false,
            &make_sets(&["C1"]),
            &HashSet::new(),
            "",
            &[]
        ));
    }

    #[test]
    fn gating_falls_back_to_global_true() {
        assert!(resolve_mention_gating(
            &SessionType::Channel,
            "C2",
            None,
            true,
            &make_sets(&["C1"]),
            &make_sets(&["C3"]),
            "",
            &[]
        ));
    }

    #[test]
    fn gating_falls_back_to_global_false() {
        assert!(!resolve_mention_gating(
            &SessionType::Channel,
            "C2",
            None,
            false,
            &make_sets(&["C1"]),
            &make_sets(&["C3"]),
            "",
            &[]
        ));
    }

    #[test]
    fn gating_no_mention_takes_precedence_over_gating_channel() {
        assert!(!resolve_mention_gating(
            &SessionType::Channel,
            "C1",
            None,
            true,
            &make_sets(&["C1"]),
            &make_sets(&["C1"]),
            "",
            &[]
        ));
    }

    #[test]
    fn gating_pattern_match_bypasses_gating() {
        let patterns = vec!["hey bot".to_string()];
        assert!(!resolve_mention_gating(
            &SessionType::Channel,
            "C1",
            None,
            true,
            &HashSet::new(),
            &HashSet::new(),
            "hey bot can you help?",
            &patterns,
        ));
    }

    #[test]
    fn gating_pattern_case_insensitive() {
        let patterns = vec!["hey bot".to_string()];
        assert!(!resolve_mention_gating(
            &SessionType::Channel,
            "C1",
            None,
            true,
            &HashSet::new(),
            &HashSet::new(),
            "HEY BOT please help",
            &patterns,
        ));
    }

    // ── annotate_custom_emoji ───────────────────────────────────────────────

    #[test]
    fn annotate_replaces_known() {
        let mut map = HashMap::new();
        map.insert(
            "company_logo".to_string(),
            "https://emoji.example.com/logo.png".to_string(),
        );
        let result = annotate_custom_emoji("Great :company_logo: work!", &map);
        assert!(result.contains(":company_logo: [custom emoji]"));
    }

    #[test]
    fn annotate_leaves_standard_alone() {
        let mut map = HashMap::new();
        map.insert("custom_one".to_string(), "url".to_string());
        let result = annotate_custom_emoji("Hello :thumbsup: world", &map);
        assert_eq!(result, "Hello :thumbsup: world");
    }

    #[test]
    fn annotate_empty_map_returns_unchanged() {
        let result = annotate_custom_emoji("Hello :custom:", &HashMap::new());
        assert_eq!(result, "Hello :custom:");
    }
}
